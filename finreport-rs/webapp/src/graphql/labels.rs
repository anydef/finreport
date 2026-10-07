//! `Transaction.label`/`.splits` and the four label/split mutations (§2.6,
//! §5): per-transaction override, split and clear, all publish-then-upsert
//! (§2.1) against `transaction_user_label`/`transaction_split`.

use async_graphql::ErrorExtensions;
use chrono::Utc;
use entity::entities::{
    transaction, transaction_label, transaction_split, transaction_user_label,
};
use rust_decimal::Decimal as RustDecimal;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    Statement,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::graphql::categories::find_by_slug;
use crate::graphql::events::{kafka_unavailable_error, publish_event};
use crate::graphql::scalars::{Date as GqlDate, Decimal as GqlDecimal, Uuid as GqlUuid};
use crate::graphql::types::{
    LabelSource as GqlLabelSource, LabelStatus as GqlLabelStatus, ReviewReason as GqlReviewReason,
    SplitPartInput, Transaction, TransactionLabel, TransactionSplit,
};
use crate::kafka::labeling::{SplitPart, UserLabelRecord, CURRENT_SCHEMA_VERSION, TOPIC_USER_LABEL};
use crate::kafka::producer::EventPublisher;

fn validation_error(message: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(message.into()).extend_with(|_, e| e.set("code", "VALIDATION"))
}

fn not_found_error() -> async_graphql::Error {
    async_graphql::Error::new("transaction not found or not accessible")
}

// ---------------------------------------------------------------------------
// Prefetch cache (§9 N+1 avoidance)
// ---------------------------------------------------------------------------

/// Per-request cache populated up front by the list resolvers
/// (`transactions`, `reviewQueue`) so `Transaction.label`/`.splits` — both
/// per-row `#[ComplexObject]` resolvers — read from memory instead of
/// issuing one query per transaction. Inserted fresh per request by
/// `request_with_auth` (`graphql/mod.rs`); a resolver that finds nothing
/// here (e.g. a future single-transaction query) falls back to its own
/// query, so correctness never depends on the cache being warm.
#[derive(Default)]
pub struct LabelSplitCache {
    labels: RwLock<HashMap<Uuid, Option<TransactionLabel>>>,
    splits: RwLock<HashMap<Uuid, Vec<TransactionSplit>>>,
}

fn gql_label_source(source: &str) -> GqlLabelSource {
    match source {
        "user" => GqlLabelSource::User,
        "rule" => GqlLabelSource::Rule,
        "llm-cache" => GqlLabelSource::LlmCache,
        _ => GqlLabelSource::Llm,
    }
}

fn gql_review_reason(reason: &str) -> Option<GqlReviewReason> {
    match reason {
        "ambiguous" => Some(GqlReviewReason::Ambiguous),
        "new_category" => Some(GqlReviewReason::NewCategory),
        // `provider_error`/`split_mismatch` have no SDL-level enum variant
        // mapping of their own (§5 only defines AMBIGUOUS/NEW_CATEGORY);
        // surfaced as `null` rather than failing the whole row.
        _ => None,
    }
}

async fn categories_by_id(
    db: &DatabaseConnection,
    ids: &[Uuid],
) -> async_graphql::Result<HashMap<Uuid, crate::graphql::types::Category>> {
    let mut unique: Vec<Uuid> = ids.to_vec();
    unique.sort();
    unique.dedup();
    if unique.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = entity::entities::category::Entity::find()
        .filter(entity::entities::category::Column::Id.is_in(unique))
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.id, crate::graphql::categories::to_graphql(row)))
        .collect())
}

/// The category a user pinned on each of `transaction_ids`, from the
/// override layer (`transaction_user_label`), skipping rows that carry no
/// category (a tags-only or recurring-only record — §2.1).
///
/// The read path needs this because the two layers are written by different
/// processes: a mutation writes the override synchronously, while the
/// `transaction_label` projection it resolves into is rewritten by the
/// labeler on its next pass. Between the two, the projection holds a label
/// the precedence chain (§2.5) has already superseded, so reporting it
/// would answer with a *lower*-precedence source than the one on record.
async fn user_category_overrides(
    db: &DatabaseConnection,
    transaction_ids: &[Uuid],
) -> async_graphql::Result<HashMap<Uuid, crate::graphql::types::Category>> {
    if transaction_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = transaction_user_label::Entity::find()
        .filter(
            transaction_user_label::Column::TransactionId.is_in(transaction_ids.to_vec()),
        )
        .filter(transaction_user_label::Column::CategoryId.is_not_null())
        .all(db)
        .await?;
    let category_ids: Vec<Uuid> = rows.iter().filter_map(|r| r.category_id).collect();
    let categories = categories_by_id(db, &category_ids).await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let category = row.category_id.and_then(|id| categories.get(&id).cloned())?;
            Some((row.transaction_id, category))
        })
        .collect())
}

/// Transactions among `transaction_ids` the user has split. A split
/// outranks even a category override (§2.5 step 1, ahead of step 2), and
/// carries no single category of its own — the parts do.
///
/// Mirrors `projection::labeling::find_valid_splits`' notion of a split
/// that counts: parts flagged `invalid` (they no longer add up to the
/// transaction amount) are ignored, exactly as the labeler ignores them.
async fn split_transaction_ids(
    db: &DatabaseConnection,
    transaction_ids: &[Uuid],
) -> async_graphql::Result<std::collections::HashSet<Uuid>> {
    if transaction_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    Ok(transaction_split::Entity::find()
        .filter(transaction_split::Column::TransactionId.is_in(transaction_ids.to_vec()))
        .filter(transaction_split::Column::Invalid.eq(false))
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.transaction_id)
        .collect())
}

/// The label a user decision resolves to, built to match exactly what the
/// labeler's own top two branches (`labeling::processor`, §2.5 steps 1–2)
/// will write once it catches up — so the value a caller reads does not
/// change when it does. `category` is `None` for a split, which has no
/// single category.
fn user_override_label(
    category: Option<crate::graphql::types::Category>,
) -> TransactionLabel {
    TransactionLabel {
        category,
        source: GqlLabelSource::User,
        rule: None,
        confidence: None,
        status: GqlLabelStatus::Resolved,
        review_reason: None,
        proposed_category_path: None,
        reasoning: None,
    }
}

async fn categories_by_rule_id(
    db: &DatabaseConnection,
    rule_ids: &[Uuid],
) -> async_graphql::Result<HashMap<Uuid, crate::graphql::types::Rule>> {
    let mut unique: Vec<Uuid> = rule_ids.to_vec();
    unique.sort();
    unique.dedup();
    if unique.is_empty() {
        return Ok(HashMap::new());
    }
    crate::graphql::rules::rules_by_id(db, &unique).await
}

/// Batch-loads labels + splits for `transaction_ids` in a handful of fixed
/// queries (not one per transaction) and stores the GraphQL-shaped result in
/// `cache`.
pub async fn prefetch(
    db: &DatabaseConnection,
    cache: &LabelSplitCache,
    transaction_ids: &[Uuid],
) -> async_graphql::Result<()> {
    if transaction_ids.is_empty() {
        return Ok(());
    }

    let label_rows = transaction_label::Entity::find()
        .filter(transaction_label::Column::TransactionId.is_in(transaction_ids.to_vec()))
        .all(db)
        .await?;
    let category_ids: Vec<Uuid> = label_rows.iter().filter_map(|r| r.category_id).collect();
    let rule_ids: Vec<Uuid> = label_rows.iter().filter_map(|r| r.rule_id).collect();
    let categories = categories_by_id(db, &category_ids).await?;
    let rules = categories_by_rule_id(db, &rule_ids).await?;
    let overrides = user_category_overrides(db, transaction_ids).await?;
    let split_ids = split_transaction_ids(db, transaction_ids).await?;

    let mut labels = cache.labels.write().await;
    // Pre-seed every requested id with `None` so `label_for`'s cache-hit
    // check (and `splits_for`'s "was this id prefetched" check) is correct
    // even for transactions with no label row at all.
    for id in transaction_ids {
        labels.insert(*id, None);
    }
    for row in label_rows {
        let category = row.category_id.and_then(|id| categories.get(&id).cloned());
        let rule = row.rule_id.and_then(|id| rules.get(&id).cloned());
        labels.insert(
            row.transaction_id,
            Some(TransactionLabel {
                category,
                source: gql_label_source(&row.label_source),
                rule,
                confidence: row.confidence.map(|c| {
                    use rust_decimal::prelude::ToPrimitive;
                    c.to_f32().unwrap_or(0.0)
                }),
                status: if row.status == "resolved" {
                    GqlLabelStatus::Resolved
                } else {
                    GqlLabelStatus::NeedsReview
                },
                review_reason: row.review_reason.as_deref().and_then(gql_review_reason),
                proposed_category_path: row.proposed_category_path,
                reasoning: row.reasoning,
            }),
        );
    }
    // Applied after the projected rows so a user decision beats them, and
    // so a transaction the labeler has not labelled at all still reports
    // what its owner chose. Splits go last of all, mirroring the chain's
    // own order: a split outranks a category override.
    for (transaction_id, category) in overrides {
        labels.insert(transaction_id, Some(user_override_label(Some(category))));
    }
    for transaction_id in split_ids {
        labels.insert(transaction_id, Some(user_override_label(None)));
    }
    drop(labels);

    let split_rows = transaction_split::Entity::find()
        .filter(transaction_split::Column::TransactionId.is_in(transaction_ids.to_vec()))
        .all(db)
        .await?;
    let split_category_ids: Vec<Uuid> = split_rows.iter().map(|r| r.category_id).collect();
    let split_categories = categories_by_id(db, &split_category_ids).await?;

    let mut splits = cache.splits.write().await;
    for row in split_rows {
        let category = split_categories
            .get(&row.category_id)
            .cloned()
            .unwrap_or_else(|| placeholder_category(row.category_id));
        splits
            .entry(row.transaction_id)
            .or_default()
            .push(TransactionSplit {
                index: row.part_index,
                amount: GqlDecimal(row.amount),
                category,
            });
    }
    Ok(())
}

fn placeholder_category(id: Uuid) -> crate::graphql::types::Category {
    crate::graphql::types::Category {
        id: GqlUuid(id),
        slug: "unknown".to_string(),
        name: "Unknown".to_string(),
        kind: crate::graphql::types::CategoryKind::Expense,
        parent_id: None,
        depth: 0,
        archived: false,
        origin: "seed".to_string(),
    }
}

/// `Transaction.label` (§5): cache hit first, a per-row fallback query
/// otherwise (correctness never depends on the cache being warm).
pub async fn label_for(
    db: &DatabaseConnection,
    cache: Option<&LabelSplitCache>,
    transaction_id: Uuid,
) -> async_graphql::Result<Option<TransactionLabel>> {
    if let Some(cache) = cache
        && let Some(found) = cache.labels.write().await.remove(&transaction_id)
    {
        return Ok(found);
    }
    // A user decision outranks whatever the projection currently holds, and
    // stands on its own when the labeler has not written a row yet. Split
    // first, then a category override — the chain's own order (§2.5).
    if split_transaction_ids(db, &[transaction_id])
        .await?
        .contains(&transaction_id)
    {
        return Ok(Some(user_override_label(None)));
    }
    if let Some(category) = user_category_overrides(db, &[transaction_id])
        .await?
        .remove(&transaction_id)
    {
        return Ok(Some(user_override_label(Some(category))));
    }
    let row = transaction_label::Entity::find_by_id(transaction_id)
        .one(db)
        .await?;
    match row {
        None => Ok(None),
        Some(row) => {
            let category = match row.category_id {
                Some(id) => categories_by_id(db, &[id]).await?.remove(&id),
                None => None,
            };
            let rule = match row.rule_id {
                Some(id) => categories_by_rule_id(db, &[id]).await?.remove(&id),
                None => None,
            };
            Ok(Some(TransactionLabel {
                category,
                source: gql_label_source(&row.label_source),
                rule,
                confidence: row.confidence.map(|c| {
                    use rust_decimal::prelude::ToPrimitive;
                    c.to_f32().unwrap_or(0.0)
                }),
                status: if row.status == "resolved" {
                    GqlLabelStatus::Resolved
                } else {
                    GqlLabelStatus::NeedsReview
                },
                review_reason: row.review_reason.as_deref().and_then(gql_review_reason),
                proposed_category_path: row.proposed_category_path,
                reasoning: row.reasoning,
            }))
        }
    }
}

/// `Transaction.splits` (§5): cache hit first, a per-row fallback query
/// otherwise.
pub async fn splits_for(
    db: &DatabaseConnection,
    cache: Option<&LabelSplitCache>,
    transaction_id: Uuid,
) -> async_graphql::Result<Vec<TransactionSplit>> {
    if let Some(cache) = cache {
        if let Some(found) = cache.splits.write().await.remove(&transaction_id) {
            return Ok(found);
        }
        // Prefetched but had zero split rows: still a legitimate cache hit,
        // distinguishable from "never prefetched" by the labels map having
        // been populated for this id.
        if cache.labels.read().await.contains_key(&transaction_id) {
            return Ok(Vec::new());
        }
    }
    let rows = transaction_split::Entity::find()
        .filter(transaction_split::Column::TransactionId.eq(transaction_id))
        .all(db)
        .await?;
    let category_ids: Vec<Uuid> = rows.iter().map(|r| r.category_id).collect();
    let categories = categories_by_id(db, &category_ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| TransactionSplit {
            index: row.part_index,
            amount: GqlDecimal(row.amount),
            category: categories
                .get(&row.category_id)
                .cloned()
                .unwrap_or_else(|| placeholder_category(row.category_id)),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

pub(crate) async fn load_scoped_transaction(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    transaction_id: Uuid,
) -> async_graphql::Result<transaction::Model> {
    transaction::Entity::find_by_id(transaction_id)
        .filter(transaction::Column::AccountId.is_in(scoped_ids.to_vec()))
        .one(db)
        .await?
        .ok_or_else(not_found_error)
}

fn to_graphql_transaction(row: &transaction::Model) -> Transaction {
    Transaction {
        id: GqlUuid(row.id),
        account_id: GqlUuid(row.account_id),
        source: row.source.clone(),
        external_id: row.external_id.clone(),
        booking_date: GqlDate(row.booking_date),
        valuta_date: row.valuta_date.map(GqlDate),
        booking_status: row.booking_status.clone(),
        amount: GqlDecimal(row.amount),
        currency: row.currency.clone(),
        counterparty_name: row.counterparty_name.clone(),
        counterparty_iban: row.counterparty_iban.clone(),
        description: row.description.clone(),
        transaction_type: row.transaction_type.clone(),
    }
}

/// Upserts `transaction_user_label`, guarded by `revision` (§2.1). Returns
/// whether the write actually applied (`false` ⇒ a newer row already won,
/// so the caller must not touch `transaction_split` either).
async fn upsert_user_label(
    db: &DatabaseConnection,
    transaction_id: Uuid,
    category_id: Option<Uuid>,
    note: Option<&str>,
    revision: chrono::DateTime<Utc>,
) -> async_graphql::Result<bool> {
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        r#"INSERT INTO transaction_user_label (transaction_id, category_id, note, revision)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (transaction_id) DO UPDATE SET
             category_id = excluded.category_id, note = excluded.note, revision = excluded.revision
           WHERE transaction_user_label.revision <= excluded.revision"#,
        vec![
            transaction_id.into(),
            category_id.into(),
            note.into(),
            revision.into(),
        ],
    );
    let result = db.execute(stmt).await?;
    Ok(result.rows_affected() > 0)
}

async fn replace_splits(
    db: &DatabaseConnection,
    transaction_id: Uuid,
    parts: &[(i32, RustDecimal, Uuid)],
) -> async_graphql::Result<()> {
    transaction_split::Entity::delete_many()
        .filter(transaction_split::Column::TransactionId.eq(transaction_id))
        .exec(db)
        .await?;
    for (index, amount, category_id) in parts {
        transaction_split::ActiveModel {
            id: sea_orm::Set(crate::kafka::labeling::split_uuid(transaction_id, *index)),
            transaction_id: sea_orm::Set(transaction_id),
            part_index: sea_orm::Set(*index),
            amount: sea_orm::Set(*amount),
            category_id: sea_orm::Set(*category_id),
            invalid: sea_orm::Set(false),
        }
        .insert(db)
        .await?;
    }
    Ok(())
}

async fn clear_splits(db: &DatabaseConnection, transaction_id: Uuid) -> async_graphql::Result<()> {
    transaction_split::Entity::delete_many()
        .filter(transaction_split::Column::TransactionId.eq(transaction_id))
        .exec(db)
        .await?;
    Ok(())
}

async fn publish_user_label(
    publisher: &Arc<EventPublisher>,
    record: &UserLabelRecord,
) -> async_graphql::Result<()> {
    let key = format!("{}:{}", record.source, record.external_id);
    let value = serde_json::to_vec(record)
        .map_err(|e| async_graphql::Error::new(format!("failed to serialize user-label: {e}")))?;
    publish_event(publisher, TOPIC_USER_LABEL, &key, &value).await
}

/// `setTransactionCategory` (§5).
pub async fn set_transaction_category(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    scoped_ids: &[Uuid],
    transaction_id: Uuid,
    category_slug: String,
) -> async_graphql::Result<Transaction> {
    let txn = load_scoped_transaction(db, scoped_ids, transaction_id).await?;
    let category = find_by_slug(db, &category_slug)
        .await?
        .ok_or_else(|| validation_error(format!("category '{category_slug}' does not exist")))?;
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    // §2.1 read-modify-write: a category change republishes the whole
    // record, so the current tags/recurring override must ride along
    // unchanged (WP0 addendum §9.7 flagged the previous `Vec::new()`/`None`
    // as a bug WP-B owned fixing).
    let (tags, recurring) =
        crate::graphql::insights::preserved_tags_and_recurring(db, transaction_id).await?;
    let revision = Utc::now();
    let record = UserLabelRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        source: txn.source.clone(),
        external_id: txn.external_id.clone(),
        category_slug: Some(category_slug),
        parts: Vec::new(),
        tags,
        recurring,
        revision,
        note: None,
    };
    publish_user_label(publisher, &record).await?;
    if upsert_user_label(db, transaction_id, Some(category.id), None, revision).await? {
        clear_splits(db, transaction_id).await?;
    }
    Ok(to_graphql_transaction(&txn))
}

/// `clearTransactionCategory` (§2.6, §5): a real event, not a tombstone —
/// falls back to the next resolution source.
pub async fn clear_transaction_category(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    scoped_ids: &[Uuid],
    transaction_id: Uuid,
) -> async_graphql::Result<Transaction> {
    let txn = load_scoped_transaction(db, scoped_ids, transaction_id).await?;
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    let (tags, recurring) =
        crate::graphql::insights::preserved_tags_and_recurring(db, transaction_id).await?;
    let revision = Utc::now();
    let record = UserLabelRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        source: txn.source.clone(),
        external_id: txn.external_id.clone(),
        category_slug: None,
        parts: Vec::new(),
        tags,
        recurring,
        revision,
        note: None,
    };
    publish_user_label(publisher, &record).await?;
    if upsert_user_label(db, transaction_id, None, None, revision).await? {
        clear_splits(db, transaction_id).await?;
    }
    Ok(to_graphql_transaction(&txn))
}

/// `unsplitTransaction` (§2.6, §5): clears the split the same way
/// `clearTransactionCategory` clears an override — there is no third shape
/// in `UserLabelRecord` for "keep the override, only drop the parts".
pub async fn unsplit_transaction(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    scoped_ids: &[Uuid],
    transaction_id: Uuid,
) -> async_graphql::Result<Transaction> {
    clear_transaction_category(db, publisher, scoped_ids, transaction_id).await
}

/// Computed delta for `SPLIT_SUM_MISMATCH` (§5): the signed difference
/// between what the parts sum to and the transaction's actual amount.
fn split_sum_mismatch_error(expected: RustDecimal, actual_sum: RustDecimal) -> async_graphql::Error {
    let delta = expected - actual_sum;
    async_graphql::Error::new(format!(
        "split parts sum to {actual_sum} but the transaction amount is {expected} (delta {delta})"
    ))
    .extend_with(|_, e| {
        e.set("code", "SPLIT_SUM_MISMATCH");
        e.set("delta", delta.to_string());
    })
}

/// `splitTransaction` (§2.6, §5): ≥ 2 parts, every part non-zero and the
/// same sign as the transaction, parts sum **exactly** to the transaction
/// amount at `NUMERIC(20,4)` — no tolerance.
pub async fn split_transaction(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    scoped_ids: &[Uuid],
    transaction_id: Uuid,
    parts: Vec<SplitPartInput>,
) -> async_graphql::Result<Transaction> {
    let txn = load_scoped_transaction(db, scoped_ids, transaction_id).await?;

    if txn.amount.is_zero() {
        return Err(validation_error("a zero-amount transaction cannot be split"));
    }
    if parts.len() < 2 {
        return Err(validation_error("a split needs at least 2 parts"));
    }

    let mut sum = RustDecimal::ZERO;
    let mut resolved_parts: Vec<(i32, RustDecimal, Uuid, String)> = Vec::with_capacity(parts.len());
    for (index, part) in parts.iter().enumerate() {
        let amount = part.amount.0;
        if amount.is_zero() {
            return Err(validation_error(format!("part {index} has a zero amount")));
        }
        if amount.is_sign_positive() != txn.amount.is_sign_positive() {
            return Err(validation_error(format!(
                "part {index} has a different sign than the transaction"
            )));
        }
        let category = find_by_slug(db, &part.category_slug).await?.ok_or_else(|| {
            validation_error(format!(
                "category '{}' does not exist",
                part.category_slug
            ))
        })?;
        sum += amount;
        resolved_parts.push((index as i32, amount, category.id, part.category_slug.clone()));
    }
    if sum != txn.amount {
        return Err(split_sum_mismatch_error(txn.amount, sum));
    }

    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let (tags, recurring) =
        crate::graphql::insights::preserved_tags_and_recurring(db, transaction_id).await?;
    let revision = Utc::now();
    let record = UserLabelRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        source: txn.source.clone(),
        external_id: txn.external_id.clone(),
        category_slug: None,
        parts: resolved_parts
            .iter()
            .map(|(index, amount, _, category_slug)| SplitPart {
                index: *index,
                amount: *amount,
                category_slug: category_slug.clone(),
            })
            .collect(),
        tags,
        recurring,
        revision,
        note: None,
    };
    publish_user_label(publisher, &record).await?;
    if upsert_user_label(db, transaction_id, None, None, revision).await? {
        let insertable: Vec<(i32, RustDecimal, Uuid)> = resolved_parts
            .into_iter()
            .map(|(index, amount, category_id, _)| (index, amount, category_id))
            .collect();
        replace_splits(db, transaction_id, &insertable).await?;
    }
    Ok(to_graphql_transaction(&txn))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_sum_mismatch_reports_signed_delta() {
        let err = split_sum_mismatch_error(
            RustDecimal::from_str_exact("-50.0000").unwrap(),
            RustDecimal::from_str_exact("-49.0000").unwrap(),
        );
        let extensions = err.extensions.unwrap();
        assert_eq!(
            extensions.get("code"),
            Some(&async_graphql::Value::String("SPLIT_SUM_MISMATCH".to_string()))
        );
        assert_eq!(
            extensions.get("delta"),
            Some(&async_graphql::Value::String("-1.0000".to_string()))
        );
    }
}
