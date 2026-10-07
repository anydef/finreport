//! Rules (§2.7, §5): CRUD, state transitions and `reapplyRule`. Distinct
//! from `webapp::labeling::rules` (WP2's matching/specificity engine, out of
//! scope here) — this module only validates, publishes and projects.

use async_graphql::ErrorExtensions;
use chrono::Utc;
use entity::entities::rule;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Statement,
};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::graphql::categories::find_by_slug;
use crate::graphql::events::{kafka_unavailable_error, publish_event};
use crate::graphql::scalars::Uuid as GqlUuid;
use crate::graphql::types::{Category, Rule, RuleInput, RuleOrigin as GqlRuleOrigin, RuleState as GqlRuleState};
use crate::kafka::labeling::{
    LabelRequestRecord, LabelRequestTarget, RuleConditions, RuleOrigin as KafkaRuleOrigin,
    RuleRecord, RuleState as KafkaRuleState, CURRENT_SCHEMA_VERSION, TOPIC_LABEL_REQUEST,
    TOPIC_RULE,
};
use crate::kafka::producer::EventPublisher;

fn validation_error(message: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(message.into()).extend_with(|_, e| e.set("code", "VALIDATION"))
}

/// The only keys §2.7 defines for `conditions`. An unknown key is rejected,
/// not silently dropped (§5).
const ALLOWED_CONDITION_KEYS: &[&str] = &[
    "counterparty_key",
    "counterparty_iban",
    "description_regex",
    "description_contains",
    "direction",
    "amount_min",
    "amount_max",
    "account_ids",
];

/// Best-effort regex syntax check (balanced delimiters + a length cap): no
/// `regex` crate is available to `webapp` without touching the frozen
/// `Cargo.toml`, so this cannot fully validate `description_regex`, only
/// catch the common malformed cases (unbalanced `()`/`[]`, absurd length).
/// Documented as a WP4 deviation from §2.7's "compiled with a size limit".
fn looks_like_valid_regex(pattern: &str) -> bool {
    if pattern.len() > 500 {
        return false;
    }
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // An escape consumes the next char; a trailing lone `\` is
                // invalid.
                if chars.next().is_none() {
                    return false;
                }
            }
            '(' => paren_depth += 1,
            ')' => {
                paren_depth -= 1;
                if paren_depth < 0 {
                    return false;
                }
            }
            '[' => bracket_depth += 1,
            ']' => {
                bracket_depth -= 1;
                if bracket_depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    paren_depth == 0 && bracket_depth == 0
}

fn validate_conditions(value: &serde_json::Value) -> async_graphql::Result<RuleConditions> {
    let object = value
        .as_object()
        .ok_or_else(|| validation_error("conditions must be a JSON object"))?;
    for key in object.keys() {
        if !ALLOWED_CONDITION_KEYS.contains(&key.as_str()) {
            return Err(validation_error(format!(
                "unknown rule condition key '{key}'"
            )));
        }
    }
    let conditions: RuleConditions = serde_json::from_value(value.clone())
        .map_err(|e| validation_error(format!("invalid conditions: {e}")))?;

    if let Some(pattern) = &conditions.description_regex
        && !looks_like_valid_regex(pattern)
    {
        return Err(validation_error(format!(
            "description_regex '{pattern}' does not look like a valid regex"
        )));
    }
    if let Some(direction) = &conditions.direction
        && direction != "INCOME"
        && direction != "SPENDING"
    {
        return Err(validation_error(
            "direction must be \"INCOME\" or \"SPENDING\"",
        ));
    }
    Ok(conditions)
}

fn gql_state_to_kafka(state: GqlRuleState) -> KafkaRuleState {
    match state {
        GqlRuleState::Active => KafkaRuleState::Active,
        GqlRuleState::InReview => KafkaRuleState::InReview,
        GqlRuleState::Revoked => KafkaRuleState::Revoked,
        GqlRuleState::Rejected => KafkaRuleState::Rejected,
    }
}

fn kafka_state_str(state: KafkaRuleState) -> &'static str {
    match state {
        KafkaRuleState::Active => "active",
        KafkaRuleState::InReview => "in_review",
        KafkaRuleState::Revoked => "revoked",
        KafkaRuleState::Rejected => "rejected",
    }
}

fn parse_state(state: &str) -> GqlRuleState {
    match state {
        "active" => GqlRuleState::Active,
        "in_review" => GqlRuleState::InReview,
        "revoked" => GqlRuleState::Revoked,
        _ => GqlRuleState::Rejected,
    }
}

fn parse_origin(origin: &str) -> GqlRuleOrigin {
    match origin {
        "learned" => GqlRuleOrigin::Learned,
        _ => GqlRuleOrigin::User,
    }
}

fn kafka_origin_str(origin: KafkaRuleOrigin) -> &'static str {
    match origin {
        KafkaRuleOrigin::User => "user",
        KafkaRuleOrigin::Learned => "learned",
    }
}

fn evidence_count(evidence: &Option<serde_json::Value>) -> i32 {
    match evidence {
        Some(serde_json::Value::Array(items)) => items.len() as i32,
        Some(_) => 1,
        None => 0,
    }
}

fn to_graphql(row: rule::Model, category: Category) -> Rule {
    Rule {
        id: GqlUuid(row.id),
        name: row.name,
        category,
        conditions: async_graphql::types::Json(row.conditions),
        priority: row.priority,
        state: parse_state(&row.state),
        origin: parse_origin(&row.origin),
        auto_approved: row.auto_approved,
        confidence: row.confidence.map(|c| {
            use rust_decimal::prelude::ToPrimitive;
            c.to_f32().unwrap_or(0.0)
        }),
        evidence_count: evidence_count(&row.evidence),
        created_at: crate::graphql::scalars::DateTime(row.created_at.with_timezone(&Utc)),
    }
}

/// Batch-resolves the `category` field for a page of rules in one query
/// (no N+1, §9).
async fn categories_by_id(
    db: &DatabaseConnection,
    ids: &[Uuid],
) -> async_graphql::Result<HashMap<Uuid, Category>> {
    let mut out = HashMap::new();
    // Dedup first: one query for the whole page, not one per rule (§9 N+1
    // avoidance).
    let mut unique: Vec<Uuid> = ids.to_vec();
    unique.sort();
    unique.dedup();
    if unique.is_empty() {
        return Ok(out);
    }
    let rows = entity::entities::category::Entity::find()
        .filter(entity::entities::category::Column::Id.is_in(unique))
        .all(db)
        .await?;
    for row in rows {
        out.insert(row.id, crate::graphql::categories::to_graphql(row));
    }
    // Any id missing from the projection (not yet projected, §3) falls back
    // to a synthetic placeholder rather than failing the whole list.
    for id in ids {
        out.entry(*id).or_insert_with(|| placeholder_category(*id));
    }
    Ok(out)
}

fn placeholder_category(id: Uuid) -> Category {
    Category {
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

/// Batch-resolves rules by id for `TransactionLabel.rule` (§9 N+1
/// avoidance) — used by `labels.rs`'s prefetch and per-row fallback.
pub async fn rules_by_id(
    db: &DatabaseConnection,
    ids: &[Uuid],
) -> async_graphql::Result<HashMap<Uuid, Rule>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = rule::Entity::find()
        .filter(rule::Column::Id.is_in(ids.to_vec()))
        .all(db)
        .await?;
    let category_ids: Vec<Uuid> = rows.iter().map(|r| r.category_id).collect();
    let categories = categories_by_id(db, &category_ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let category = categories
                .get(&row.category_id)
                .cloned()
                .unwrap_or_else(|| placeholder_category(row.category_id));
            let id = row.id;
            (id, to_graphql(row, category))
        })
        .collect())
}

pub async fn fetch_rules(
    db: &DatabaseConnection,
    state: Option<GqlRuleState>,
) -> async_graphql::Result<Vec<Rule>> {
    let mut query = rule::Entity::find().order_by_desc(rule::Column::CreatedAt);
    if let Some(state) = state {
        query = query.filter(rule::Column::State.eq(kafka_state_str(gql_state_to_kafka(state))));
    }
    let rows = query.all(db).await?;
    let category_ids: Vec<Uuid> = rows.iter().map(|r| r.category_id).collect();
    let categories = categories_by_id(db, &category_ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let category = categories
                .get(&row.category_id)
                .cloned()
                .unwrap_or_else(|| placeholder_category(row.category_id));
            to_graphql(row, category)
        })
        .collect())
}

pub async fn fetch_recently_auto_approved(
    db: &DatabaseConnection,
    limit: i32,
) -> async_graphql::Result<Vec<Rule>> {
    let limit = limit.clamp(1, 200) as u64;
    let rows = rule::Entity::find()
        .filter(rule::Column::AutoApproved.eq(true))
        .order_by_desc(rule::Column::CreatedAt)
        .limit(limit)
        .all(db)
        .await?;
    let category_ids: Vec<Uuid> = rows.iter().map(|r| r.category_id).collect();
    let categories = categories_by_id(db, &category_ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let category = categories
                .get(&row.category_id)
                .cloned()
                .unwrap_or_else(|| placeholder_category(row.category_id));
            to_graphql(row, category)
        })
        .collect())
}

/// Upserts the `rule` projection, guarded by `revision` (§2.1).
async fn upsert_rule(db: &DatabaseConnection, record: &RuleRecord) -> async_graphql::Result<()> {
    let category = find_by_slug(db, &record.category_slug)
        .await?
        .ok_or_else(|| {
            validation_error(format!(
                "category '{}' does not exist",
                record.category_slug
            ))
        })?;
    let conditions_json = serde_json::to_value(&record.conditions)
        .map_err(|e| async_graphql::Error::new(format!("failed to serialize conditions: {e}")))?;

    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        r#"INSERT INTO rule
             (id, name, category_id, conditions, priority, state, origin, auto_approved,
              user_touched, confidence, evidence, created_at, revision)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
           ON CONFLICT (id) DO UPDATE SET
             name = excluded.name, category_id = excluded.category_id,
             conditions = excluded.conditions, priority = excluded.priority,
             state = excluded.state, origin = excluded.origin,
             auto_approved = excluded.auto_approved, user_touched = excluded.user_touched,
             confidence = excluded.confidence, evidence = excluded.evidence,
             revision = excluded.revision
           WHERE rule.revision <= excluded.revision"#,
        vec![
            record.id.into(),
            record.name.clone().into(),
            category.id.into(),
            conditions_json.into(),
            record.priority.into(),
            kafka_state_str(record.state).into(),
            kafka_origin_str(record.origin).into(),
            record.auto_approved.into(),
            record.user_touched.into(),
            record.confidence.map(|c| c as f64).into(),
            record.evidence.clone().into(),
            record.created_at.into(),
            record.revision.into(),
        ],
    );
    db.execute(stmt).await?;
    Ok(())
}

async fn publish_and_project(
    db: &DatabaseConnection,
    publisher: &Arc<EventPublisher>,
    record: &RuleRecord,
) -> async_graphql::Result<()> {
    let value = serde_json::to_vec(record)
        .map_err(|e| async_graphql::Error::new(format!("failed to serialize rule: {e}")))?;
    publish_event(publisher, TOPIC_RULE, &record.id.to_string(), &value).await?;
    upsert_rule(db, record).await
}

async fn load_rule(db: &DatabaseConnection, id: Uuid) -> async_graphql::Result<Rule> {
    let row = rule::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| async_graphql::Error::new("rule upsert did not take effect"))?;
    let category = categories_by_id(db, &[row.category_id])
        .await?
        .remove(&row.category_id)
        .unwrap_or_else(|| placeholder_category(row.category_id));
    Ok(to_graphql(row, category))
}

/// `createRule` (§2.7, §5): always `origin = user`, `user_touched = true` so
/// the learner never overwrites it.
pub async fn create_rule(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    input: RuleInput,
) -> async_graphql::Result<Rule> {
    let conditions = validate_conditions(&input.conditions.0)?;
    if find_by_slug(db, &input.category_slug).await?.is_none() {
        return Err(validation_error(format!(
            "category '{}' does not exist",
            input.category_slug
        )));
    }
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    let id = Uuid::new_v4();
    let now = Utc::now();
    let record = RuleRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        id,
        name: input.name,
        category_slug: input.category_slug,
        conditions,
        priority: input.priority,
        state: KafkaRuleState::Active,
        origin: KafkaRuleOrigin::User,
        auto_approved: false,
        user_touched: true,
        confidence: None,
        evidence: None,
        created_at: now,
        revision: now,
    };
    publish_and_project(db, publisher, &record).await?;
    load_rule(db, id).await
}

/// `updateRule` (§2.7, §5): republishes the full record with the caller's
/// new fields; `user_touched` stays `true`.
pub async fn update_rule(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    id: Uuid,
    input: RuleInput,
) -> async_graphql::Result<Rule> {
    let existing = rule::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| async_graphql::Error::new("rule not found"))?;
    let conditions = validate_conditions(&input.conditions.0)?;
    if find_by_slug(db, &input.category_slug).await?.is_none() {
        return Err(validation_error(format!(
            "category '{}' does not exist",
            input.category_slug
        )));
    }
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    let record = RuleRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        id,
        name: input.name,
        category_slug: input.category_slug,
        conditions,
        priority: input.priority,
        state: parse_kafka_state(&existing.state),
        origin: parse_kafka_origin(&existing.origin),
        auto_approved: existing.auto_approved,
        user_touched: true,
        confidence: existing.confidence.map(|c| {
            use rust_decimal::prelude::ToPrimitive;
            c.to_f32().unwrap_or(0.0)
        }),
        evidence: existing.evidence.clone(),
        created_at: existing.created_at.with_timezone(&Utc),
        revision: Utc::now(),
    };
    publish_and_project(db, publisher, &record).await?;
    load_rule(db, id).await
}

fn parse_kafka_state(state: &str) -> KafkaRuleState {
    match state {
        "active" => KafkaRuleState::Active,
        "in_review" => KafkaRuleState::InReview,
        "revoked" => KafkaRuleState::Revoked,
        _ => KafkaRuleState::Rejected,
    }
}

fn parse_kafka_origin(origin: &str) -> KafkaRuleOrigin {
    match origin {
        "learned" => KafkaRuleOrigin::Learned,
        _ => KafkaRuleOrigin::User,
    }
}

/// `setRuleState` (§5): a revoke makes the labeler re-resolve every
/// transaction it had labelled — that is the labeler's job (§2.3), this
/// mutation only republishes the rule's state.
pub async fn set_rule_state(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    id: Uuid,
    state: GqlRuleState,
) -> async_graphql::Result<Rule> {
    let existing = rule::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| async_graphql::Error::new("rule not found"))?;
    let category = find_by_slug_for_rule(db, &existing).await?;
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    let record = RuleRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        id,
        name: existing.name.clone(),
        category_slug: category,
        conditions: serde_json::from_value(existing.conditions.clone()).unwrap_or_default(),
        priority: existing.priority,
        state: gql_state_to_kafka(state),
        origin: parse_kafka_origin(&existing.origin),
        auto_approved: existing.auto_approved,
        user_touched: true,
        confidence: existing.confidence.map(|c| {
            use rust_decimal::prelude::ToPrimitive;
            c.to_f32().unwrap_or(0.0)
        }),
        evidence: existing.evidence.clone(),
        created_at: existing.created_at.with_timezone(&Utc),
        revision: Utc::now(),
    };
    publish_and_project(db, publisher, &record).await?;
    load_rule(db, id).await
}

async fn find_by_slug_for_rule(
    db: &DatabaseConnection,
    existing: &rule::Model,
) -> async_graphql::Result<String> {
    Ok(entity::entities::category::Entity::find_by_id(existing.category_id)
        .one(db)
        .await?
        .map(|c| c.slug)
        .unwrap_or_else(|| "unknown".to_string()))
}

/// `reapplyRule` (§5): publishes one `label-request` keyed
/// `reapply:<rule-id>` and returns the count of transactions it currently
/// covers, scoped to the caller's accounts. Never drives the labeler
/// directly — re-resolution is asynchronous (§2.3).
pub async fn reapply_rule(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    scoped_ids: &[Uuid],
    id: Uuid,
) -> async_graphql::Result<i32> {
    if rule::Entity::find_by_id(id).one(db).await?.is_none() {
        return Err(async_graphql::Error::new("rule not found"));
    }
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    let record = LabelRequestRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        target: LabelRequestTarget::Rule { rule_id: id },
        requested_at: Utc::now(),
    };
    let value = serde_json::to_vec(&record).map_err(|e| {
        async_graphql::Error::new(format!("failed to serialize label-request: {e}"))
    })?;
    publish_event(
        publisher,
        TOPIC_LABEL_REQUEST,
        &format!("reapply:{id}"),
        &value,
    )
    .await?;

    if scoped_ids.is_empty() {
        return Ok(0);
    }
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        r#"SELECT COUNT(*) AS count
           FROM transaction_label tl
           JOIN transaction t ON t.id = tl.transaction_id
           WHERE tl.rule_id = $1 AND t.account_id = ANY($2)"#,
        vec![id.into(), scoped_ids.to_vec().into()],
    );
    let row = db
        .query_one(stmt)
        .await?
        .ok_or_else(|| async_graphql::Error::new("count query returned no rows"))?;
    let count: i64 = row.try_get("", "count")?;
    Ok(count as i32)
}
