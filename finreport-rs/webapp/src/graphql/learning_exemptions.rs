//! Learning exemptions: the user's decision that no rule may ever be learned
//! for a merchant (`counterparty_key`) - an Amazon order is groceries and
//! electronics and books, so a merchant-wide rule is wrong in principle.
//!
//! Like rules, exemptions are not tenant-scoped (the learner works on the
//! normalised key across every account); the caller must be authenticated and
//! is recorded as `decided_by`. What *is* scoped to the caller is the
//! evidence shown next to each exemption: the merchant's display name and
//! transaction count are computed over the caller's own accounts only.
//!
//! Exempting also discards the merchant's learned rules, except any the user
//! has touched (approved, edited, created): see [`exempt_from_learning`].
//! Lifting the exemption (a tombstone) simply lets the learner work again.

use async_graphql::{ErrorExtensions, SimpleObject};
use chrono::Utc;
use entity::entities::{learning_exemption, transaction};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::graphql::events::{kafka_unavailable_error, publish_event, publish_tombstone};
use crate::graphql::scalars::DateTime as GqlDateTime;
use crate::kafka::labeling::{
    LearningExemptionRecord, CURRENT_SCHEMA_VERSION, TOPIC_LEARNING_EXEMPTION, TOPIC_RULE,
};
use crate::kafka::producer::EventPublisher;
use crate::projection::labeling as proj;

/// One merchant the learner has been told to leave alone.
#[derive(SimpleObject, Clone, Debug)]
pub struct LearningExemption {
    /// The normalised merchant key rules and the learner index on.
    pub counterparty_key: String,
    /// The merchant's name as it appears on the caller's transactions (the
    /// most frequent spelling), falling back to the key when the caller has
    /// none.
    pub display_name: String,
    /// How many of the caller's transactions carry this merchant key.
    pub transaction_count: i32,
    pub exempted_at: GqlDateTime,
}

fn validation_error(message: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(message.into()).extend_with(|_, e| e.set("code", "VALIDATION"))
}

/// The key the learner indexes on for whatever the user typed or clicked: an
/// already-normalised key passes through unchanged, a raw name ("Amazon
/// Payments Europe S.C.A.") is normalised exactly like a transaction's
/// counterparty is. Empty after normalising is rejected.
fn canonical_key(input: &str) -> async_graphql::Result<String> {
    let key = crate::labeling::normalize::normalize(Some(input), None);
    if key.is_empty() {
        return Err(validation_error("counterpartyKey must not be empty"));
    }
    Ok(key)
}

/// Display name + count per merchant key over the caller's accounts, in one
/// query for the whole list.
async fn evidence_for(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    keys: &[String],
) -> async_graphql::Result<HashMap<String, (String, i32)>> {
    if scoped_ids.is_empty() || keys.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = transaction::Entity::find()
        .select_only()
        .columns([
            transaction::Column::CounterpartyKey,
            transaction::Column::CounterpartyName,
        ])
        .filter(transaction::Column::AccountId.is_in(scoped_ids.to_vec()))
        .filter(transaction::Column::CounterpartyKey.is_in(keys.to_vec()))
        .into_tuple::<(Option<String>, Option<String>)>()
        .all(db)
        .await?;
    let mut names: HashMap<String, HashMap<String, i32>> = HashMap::new();
    let mut counts: HashMap<String, i32> = HashMap::new();
    for (key, name) in rows {
        let Some(key) = key else { continue };
        *counts.entry(key.clone()).or_default() += 1;
        if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
            *names.entry(key).or_default().entry(name).or_default() += 1;
        }
    }
    Ok(counts
        .into_iter()
        .map(|(key, count)| {
            let name = names
                .get(&key)
                .and_then(|spellings| {
                    // Most frequent spelling; ties broken alphabetically so the
                    // answer is stable.
                    spellings
                        .iter()
                        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                        .map(|(n, _)| n.clone())
                })
                .unwrap_or_else(|| key.clone());
            (key, (name, count))
        })
        .collect())
}

fn to_graphql(
    row: learning_exemption::Model,
    evidence: &HashMap<String, (String, i32)>,
) -> LearningExemption {
    let (display_name, transaction_count) = evidence
        .get(&row.counterparty_key)
        .cloned()
        .unwrap_or_else(|| (row.counterparty_key.clone(), 0));
    LearningExemption {
        counterparty_key: row.counterparty_key,
        display_name,
        transaction_count,
        exempted_at: GqlDateTime(row.revision.with_timezone(&Utc)),
    }
}

/// `learningExemptions`: every exempt merchant, most recent first.
pub async fn fetch_learning_exemptions(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
) -> async_graphql::Result<Vec<LearningExemption>> {
    let rows = learning_exemption::Entity::find()
        .order_by_desc(learning_exemption::Column::Revision)
        .all(db)
        .await?;
    let keys: Vec<String> = rows.iter().map(|r| r.counterparty_key.clone()).collect();
    let evidence = evidence_for(db, scoped_ids, &keys).await?;
    Ok(rows.into_iter().map(|r| to_graphql(r, &evidence)).collect())
}

/// `exemptFromLearning`: publish the exemption, project it, then discard the
/// merchant's learned rules.
///
/// Order matters: the exemption is projected *before* any rule tombstone is
/// published, because the labeler re-resolves a tombstoned rule's labels and
/// would otherwise be free to learn the rule straight back.
///
/// Which rules go: those with `origin = learned` and `user_touched = false`
/// whose conditions name this merchant. They are tombstoned (not merely
/// revoked) so that lifting the exemption later lets the learner start from
/// a clean slate - a revoked learned rule is never re-learned. Labels they
/// set fall back to the next source in the chain. A rule the user created,
/// edited, approved or rejected (`user_touched`) is their own work and stays
/// exactly as it is.
pub async fn exempt_from_learning(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    scoped_ids: &[Uuid],
    decided_by: Uuid,
    counterparty_key: &str,
) -> async_graphql::Result<LearningExemption> {
    let key = canonical_key(counterparty_key)?;
    let key = key.as_str();
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    let record = LearningExemptionRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        counterparty_key: key.to_string(),
        decided_by: Some(decided_by),
        revision: Utc::now(),
    };
    let value = serde_json::to_vec(&record).map_err(|e| {
        async_graphql::Error::new(format!("failed to serialize learning exemption: {e}"))
    })?;
    publish_event(publisher, TOPIC_LEARNING_EXEMPTION, key, &value).await?;
    proj::project_learning_exemption(db, key, Some(record)).await?;

    for rule_id in proj::discardable_learned_rule_ids(db, key).await? {
        publish_tombstone(publisher, TOPIC_RULE, &rule_id.to_string()).await?;
        proj::project_rule(db, rule_id, None).await?;
    }

    let row = learning_exemption::Entity::find_by_id(key.to_string())
        .one(db)
        .await?
        .ok_or_else(|| async_graphql::Error::new("learning exemption did not take effect"))?;
    let evidence = evidence_for(db, scoped_ids, &[key.to_string()]).await?;
    Ok(to_graphql(row, &evidence))
}

/// `removeLearningExemption`: a tombstone, then the matching delete. Returns
/// whether an exemption existed.
pub async fn remove_learning_exemption(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    counterparty_key: &str,
) -> async_graphql::Result<bool> {
    let key = canonical_key(counterparty_key)?;
    let key = key.as_str();
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let existed = proj::is_learning_exempt(db, key).await?;
    publish_tombstone(publisher, TOPIC_LEARNING_EXEMPTION, key).await?;
    proj::project_learning_exemption(db, key, None).await?;
    Ok(existed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_key_passes_through_unchanged() {
        for key in ["amazon", "fitness first", "paypal"] {
            assert_eq!(canonical_key(key).unwrap(), key);
        }
    }

    #[test]
    fn a_typed_name_is_normalised_like_a_counterparty() {
        assert_eq!(canonical_key("  Amazon Payments GmbH ").unwrap(), "amazon payments");
        assert_eq!(canonical_key("PayPal").unwrap(), "paypal");
    }

    #[test]
    fn an_empty_key_is_rejected() {
        assert!(canonical_key("   ").is_err());
    }
}
