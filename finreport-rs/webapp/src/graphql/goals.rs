//! Iteration 4 §4: the goals GraphQL surface — `goals`, `goal`,
//! `goalProgress`, `goalTransactions` and the `createGoal`/`updateGoal`/
//! `archiveGoal` mutations.
//!
//! Every goal is scoped to its `owner_user_id`: another user's id reads as
//! `null` (where the field is optional) or `NOT_FOUND`, never as data.
//! Progress comes from `crate::goals` (WP-A); the mutations publish a
//! whole-state `GoalRecord` to `finreport.goal` and then apply the same
//! revision-guarded upsert to the `goal` projection — the publish-then-upsert
//! shape `rules.rs` uses, on `events::publish_event`. `queries.rs`/
//! `mutations.rs` delegate to these free functions.

use async_graphql::{Enum, ErrorExtensions, InputObject, SimpleObject};
use chrono::Utc;
use entity::entities::{category, goal, transaction};
use rust_decimal::Decimal as RustDecimal;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    Statement,
};
use std::collections::HashMap;
use std::sync::Arc;

use crate::goals::{
    evaluate_goal, goal_transaction_ids, BucketProgress, GoalProgressData, ProgressWindow,
};
use crate::graphql::categories::find_by_slug;
use crate::graphql::current_user::{scoped_account_ids, AuthenticatedUser};
use crate::graphql::events::{kafka_unavailable_error, publish_event};
use crate::graphql::insights::normalize_tags;
use crate::graphql::scalars::{Date, Decimal, Uuid};
use crate::graphql::transactions::{clamp_limit, to_graphql_transaction};
use crate::graphql::types::{Category, PageInput, TransactionPage};
use crate::kafka::goals as wire;
use crate::kafka::producer::EventPublisher;

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum GoalType {
    SpendingLimit,
    SavingTarget,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum GoalPeriodKind {
    Recurring,
    Fixed,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum GoalCadence {
    Monthly,
    Quarterly,
    Yearly,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ScopeCombine {
    All,
    Any,
}

#[derive(SimpleObject)]
pub struct GoalScope {
    /// Resolved from slugs; archived ones are still listed.
    pub categories: Vec<Category>,
    pub tags: Vec<String>,
    pub combine: ScopeCombine,
    pub tag_combine: ScopeCombine,
}

#[derive(SimpleObject)]
pub struct Goal {
    pub id: Uuid,
    pub name: String,
    #[graphql(name = "type")]
    pub goal_type: GoalType,
    pub amount: Decimal,
    pub currency: String,
    pub scope: GoalScope,
    pub period_kind: GoalPeriodKind,
    /// `null` for `FIXED`.
    pub cadence: Option<GoalCadence>,
    /// `null` for `RECURRING`.
    pub start_date: Option<Date>,
    /// `null` for `RECURRING` or an open-ended `FIXED`.
    pub end_date: Option<Date>,
    pub archived: bool,
}

#[derive(SimpleObject)]
pub struct GoalBucket {
    pub start: Date,
    pub end: Date,
    pub label: String,
    /// Positive magnitude.
    pub total: Decimal,
    /// Held-for-review, excluded from `total`.
    pub pending: Decimal,
    /// `amount - total`; negative when over.
    pub remaining: Decimal,
    /// `<= amount` for a limit, `>= amount` for a target.
    pub met: bool,
    pub in_progress: bool,
}

#[derive(SimpleObject)]
pub struct GoalProgress {
    pub goal: Goal,
    pub buckets: Vec<GoalBucket>,
    /// Across every bucket.
    pub total: Decimal,
    pub pending: Decimal,
    /// Over completed buckets only.
    pub average_per_period: Decimal,
    pub currency: String,
}

/// Mirrors the `finreport.goal` event's scope and period (§2.1). Validated
/// at mutation time (§4): amount > 0; a non-empty scope; `RECURRING` requires
/// `cadence` and rejects `startDate`/`endDate`; `FIXED` requires `startDate`
/// and rejects `cadence`; `endDate >= startDate`; every category slug exists;
/// tags normalized with iteration 3's rules.
#[derive(InputObject)]
pub struct GoalInput {
    pub name: String,
    #[graphql(name = "type")]
    pub goal_type: GoalType,
    pub amount: Decimal,
    #[graphql(default = "EUR")]
    pub currency: String,
    #[graphql(default)]
    pub category_slugs: Vec<String>,
    #[graphql(default)]
    pub tags: Vec<String>,
    /// How the category and tag conditions join; ignored when the scope has
    /// only one of them.
    #[graphql(default_with = "ScopeCombine::All")]
    pub combine: ScopeCombine,
    /// How the listed tags join each other.
    #[graphql(default_with = "ScopeCombine::All")]
    pub tag_combine: ScopeCombine,
    pub period_kind: GoalPeriodKind,
    pub cadence: Option<GoalCadence>,
    pub start_date: Option<Date>,
    pub end_date: Option<Date>,
}


// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn validation_error(message: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(message.into()).extend_with(|_, e| e.set("code", "VALIDATION"))
}

/// A goal that does not exist and a goal that belongs to someone else give
/// the same answer, so an id never confirms another user's goal.
fn not_found() -> async_graphql::Error {
    async_graphql::Error::new("goal not found").extend_with(|_, e| e.set("code", "NOT_FOUND"))
}

// ---------------------------------------------------------------------------
// Enum mapping (GraphQL <-> wire <-> projection strings)
// ---------------------------------------------------------------------------

impl From<GoalType> for wire::GoalType {
    fn from(v: GoalType) -> Self {
        match v {
            GoalType::SpendingLimit => Self::SpendingLimit,
            GoalType::SavingTarget => Self::SavingTarget,
        }
    }
}
impl From<ScopeCombine> for wire::Combine {
    fn from(v: ScopeCombine) -> Self {
        match v {
            ScopeCombine::All => Self::All,
            ScopeCombine::Any => Self::Any,
        }
    }
}
impl From<GoalCadence> for wire::Cadence {
    fn from(v: GoalCadence) -> Self {
        match v {
            GoalCadence::Monthly => Self::Monthly,
            GoalCadence::Quarterly => Self::Quarterly,
            GoalCadence::Yearly => Self::Yearly,
        }
    }
}

fn type_str(v: wire::GoalType) -> &'static str {
    match v {
        wire::GoalType::SpendingLimit => "spending_limit",
        wire::GoalType::SavingTarget => "saving_target",
    }
}
fn combine_str(v: wire::Combine) -> &'static str {
    match v {
        wire::Combine::All => "all",
        wire::Combine::Any => "any",
    }
}
fn cadence_str(v: wire::Cadence) -> &'static str {
    match v {
        wire::Cadence::Monthly => "monthly",
        wire::Cadence::Quarterly => "quarterly",
        wire::Cadence::Yearly => "yearly",
    }
}
fn kind_str(v: wire::PeriodKind) -> &'static str {
    match v {
        wire::PeriodKind::Recurring => "recurring",
        wire::PeriodKind::Fixed => "fixed",
    }
}

fn parse_type(s: &str) -> wire::GoalType {
    if s == "saving_target" {
        wire::GoalType::SavingTarget
    } else {
        wire::GoalType::SpendingLimit
    }
}
fn parse_combine(s: &str) -> wire::Combine {
    if s == "any" {
        wire::Combine::Any
    } else {
        wire::Combine::All
    }
}
fn parse_cadence(s: &str) -> Option<wire::Cadence> {
    match s {
        "monthly" => Some(wire::Cadence::Monthly),
        "quarterly" => Some(wire::Cadence::Quarterly),
        "yearly" => Some(wire::Cadence::Yearly),
        _ => None,
    }
}
fn parse_kind(s: &str) -> wire::PeriodKind {
    if s == "fixed" {
        wire::PeriodKind::Fixed
    } else {
        wire::PeriodKind::Recurring
    }
}

fn gql_type(v: wire::GoalType) -> GoalType {
    match v {
        wire::GoalType::SpendingLimit => GoalType::SpendingLimit,
        wire::GoalType::SavingTarget => GoalType::SavingTarget,
    }
}
fn gql_combine(v: wire::Combine) -> ScopeCombine {
    match v {
        wire::Combine::All => ScopeCombine::All,
        wire::Combine::Any => ScopeCombine::Any,
    }
}
fn gql_cadence(v: wire::Cadence) -> GoalCadence {
    match v {
        wire::Cadence::Monthly => GoalCadence::Monthly,
        wire::Cadence::Quarterly => GoalCadence::Quarterly,
        wire::Cadence::Yearly => GoalCadence::Yearly,
    }
}

// ---------------------------------------------------------------------------
// Progress arithmetic (pure)
// ---------------------------------------------------------------------------

/// `amount - total`; negative when over.
pub(crate) fn remaining(amount: RustDecimal, total: RustDecimal) -> RustDecimal {
    amount - total
}

/// A limit is met at or under the threshold, a target at or over it;
/// exactly the threshold counts as met in both directions.
pub(crate) fn is_met(goal_type: GoalType, amount: RustDecimal, total: RustDecimal) -> bool {
    match goal_type {
        GoalType::SpendingLimit => total <= amount,
        GoalType::SavingTarget => total >= amount,
    }
}

fn to_bucket(goal_type: GoalType, amount: RustDecimal, b: BucketProgress) -> GoalBucket {
    GoalBucket {
        start: Date(b.start),
        end: Date(b.end),
        label: b.label,
        total: Decimal(b.total),
        pending: Decimal(b.pending),
        remaining: Decimal(remaining(amount, b.total)),
        met: is_met(goal_type, amount, b.total),
        in_progress: b.in_progress,
    }
}

fn map_progress(goal: Goal, data: GoalProgressData) -> GoalProgress {
    let (goal_type, amount) = (goal.goal_type, goal.amount.0);
    GoalProgress {
        currency: goal.currency.clone(),
        buckets: data
            .buckets
            .into_iter()
            .map(|b| to_bucket(goal_type, amount, b))
            .collect(),
        total: Decimal(data.total),
        pending: Decimal(data.pending),
        average_per_period: Decimal(data.average_per_period),
        goal,
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// A [`GoalInput`] that passed every check that needs no database.
#[derive(Debug, PartialEq)]
pub(crate) struct ValidInput {
    pub name: String,
    pub goal_type: wire::GoalType,
    pub amount: RustDecimal,
    pub currency: String,
    pub scope: wire::GoalScope,
    pub period: wire::GoalPeriod,
}

/// Pure half of the §4 validation. Category existence is checked separately
/// ([`check_categories_exist`]) because it needs the database.
pub(crate) fn validate_input(
    input: &GoalInput,
    max_tags: u32,
) -> async_graphql::Result<ValidInput> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(validation_error("name must not be empty"));
    }
    let currency = input.currency.trim().to_uppercase();
    if currency.is_empty() {
        return Err(validation_error("currency must not be empty"));
    }
    let amount = input.amount.0.round_dp(4);
    if amount <= RustDecimal::ZERO {
        return Err(validation_error("amount must be greater than zero"));
    }

    let mut category_slugs: Vec<String> = Vec::new();
    for slug in &input.category_slugs {
        let slug = slug.trim().to_string();
        if !slug.is_empty() && !category_slugs.contains(&slug) {
            category_slugs.push(slug);
        }
    }
    let tags = normalize_tags(&input.tags, max_tags)?;
    if category_slugs.is_empty() && tags.is_empty() {
        return Err(validation_error(
            "scope must name at least one category or tag",
        ));
    }

    let start = input.start_date.map(|d| d.0);
    let end = input.end_date.map(|d| d.0);
    let period = match input.period_kind {
        GoalPeriodKind::Recurring => {
            let cadence = input
                .cadence
                .ok_or_else(|| validation_error("a RECURRING goal requires a cadence"))?;
            if start.is_some() || end.is_some() {
                return Err(validation_error(
                    "a RECURRING goal must not set startDate or endDate",
                ));
            }
            wire::GoalPeriod {
                kind: wire::PeriodKind::Recurring,
                cadence: Some(cadence.into()),
                start_date: None,
                end_date: None,
            }
        }
        GoalPeriodKind::Fixed => {
            if input.cadence.is_some() {
                return Err(validation_error("a FIXED goal must not set a cadence"));
            }
            let start =
                start.ok_or_else(|| validation_error("a FIXED goal requires a startDate"))?;
            if let Some(end) = end
                && end < start
            {
                return Err(validation_error("endDate must not be before startDate"));
            }
            wire::GoalPeriod {
                kind: wire::PeriodKind::Fixed,
                cadence: None,
                start_date: Some(start),
                end_date: end,
            }
        }
    };

    Ok(ValidInput {
        name,
        goal_type: input.goal_type.into(),
        amount,
        currency,
        scope: wire::GoalScope {
            category_slugs,
            tags,
            combine: input.combine.into(),
            tag_combine: input.tag_combine.into(),
        },
        period,
    })
}

async fn check_categories_exist(
    db: &DatabaseConnection,
    slugs: &[String],
) -> async_graphql::Result<()> {
    for slug in slugs {
        if find_by_slug(db, slug).await?.is_none() {
            return Err(validation_error(format!("category '{slug}' does not exist")));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Projection <-> GraphQL
// ---------------------------------------------------------------------------

/// Resolves slugs to categories in one query, archived ones included.
async fn categories_by_slug(
    db: &DatabaseConnection,
    slugs: &[String],
) -> async_graphql::Result<HashMap<String, Category>> {
    if slugs.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = category::Entity::find()
        .filter(category::Column::Slug.is_in(slugs.to_vec()))
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.slug.clone(), crate::graphql::categories::to_graphql(row)))
        .collect())
}

fn to_graphql(row: goal::Model, categories: &HashMap<String, Category>) -> Goal {
    let resolved = row
        .scope_category_slugs
        .iter()
        .filter_map(|slug| categories.get(slug).cloned())
        .collect();
    Goal {
        id: Uuid(row.id),
        name: row.name,
        goal_type: gql_type(parse_type(&row.goal_type)),
        amount: Decimal(row.amount),
        currency: row.currency,
        scope: GoalScope {
            categories: resolved,
            tags: row.scope_tags,
            combine: gql_combine(parse_combine(&row.scope_combine)),
            tag_combine: gql_combine(parse_combine(&row.scope_tag_combine)),
        },
        period_kind: match parse_kind(&row.period_kind) {
            wire::PeriodKind::Fixed => GoalPeriodKind::Fixed,
            wire::PeriodKind::Recurring => GoalPeriodKind::Recurring,
        },
        cadence: row
            .period_cadence
            .as_deref()
            .and_then(parse_cadence)
            .map(gql_cadence),
        start_date: row.period_start.map(Date),
        end_date: row.period_end.map(Date),
        archived: row.archived,
    }
}

async fn render(db: &DatabaseConnection, row: goal::Model) -> async_graphql::Result<Goal> {
    let categories = categories_by_slug(db, &row.scope_category_slugs).await?;
    Ok(to_graphql(row, &categories))
}

/// Loads a goal owned by `user`; anyone else's reads as absent.
async fn find_owned(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    id: uuid::Uuid,
) -> async_graphql::Result<Option<goal::Model>> {
    Ok(goal::Entity::find_by_id(id)
        .filter(goal::Column::OwnerUserId.eq(user.user_id))
        .one(db)
        .await?)
}

async fn require_owned(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    id: uuid::Uuid,
) -> async_graphql::Result<goal::Model> {
    find_owned(db, user, id).await?.ok_or_else(not_found)
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

pub async fn goals(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    include_archived: bool,
) -> async_graphql::Result<Vec<Goal>> {
    let mut query = goal::Entity::find().filter(goal::Column::OwnerUserId.eq(user.user_id));
    if !include_archived {
        query = query.filter(goal::Column::Archived.eq(false));
    }
    let rows = query
        .order_by_asc(goal::Column::Name)
        .order_by_asc(goal::Column::Id)
        .all(db)
        .await?;
    let slugs: Vec<String> = rows
        .iter()
        .flat_map(|r| r.scope_category_slugs.iter().cloned())
        .collect();
    let categories = categories_by_slug(db, &slugs).await?;
    Ok(rows.into_iter().map(|r| to_graphql(r, &categories)).collect())
}

pub async fn goal(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    id: Uuid,
) -> async_graphql::Result<Option<Goal>> {
    match find_owned(db, user, id.0).await? {
        Some(row) => Ok(Some(render(db, row).await?)),
        None => Ok(None),
    }
}

/// Both bounds or neither: a half-open window has no defined meaning here.
pub(crate) fn parse_window(
    start: Option<Date>,
    end: Option<Date>,
) -> async_graphql::Result<Option<ProgressWindow>> {
    match (start, end) {
        (None, None) => Ok(None),
        (Some(s), Some(e)) => {
            if s.0 > e.0 {
                return Err(validation_error("startDate must not be after endDate"));
            }
            Ok(Some(ProgressWindow { start: s.0, end: e.0 }))
        }
        _ => Err(validation_error(
            "startDate and endDate must be given together",
        )),
    }
}

pub async fn goal_progress(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    id: Uuid,
    start_date: Option<Date>,
    end_date: Option<Date>,
) -> async_graphql::Result<GoalProgress> {
    let window = parse_window(start_date, end_date)?;
    let row = require_owned(db, user, id.0).await?;
    let scoped = scoped_account_ids(user, None)?;
    let data = evaluate_goal(db, &row, &scoped, window).await?;
    Ok(map_progress(render(db, row).await?, data))
}

pub async fn goal_transactions(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    id: Uuid,
    start_date: Date,
    end_date: Date,
    page: Option<PageInput>,
) -> async_graphql::Result<TransactionPage> {
    if start_date.0 > end_date.0 {
        return Err(validation_error("startDate must not be after endDate"));
    }
    let row = require_owned(db, user, id.0).await?;
    let scoped = scoped_account_ids(user, None)?;
    let page = page.unwrap_or_default();
    let limit = clamp_limit(page.limit) as usize;
    let offset = page.offset.max(0) as usize;

    let ids = goal_transaction_ids(db, &row, &scoped, start_date.0, end_date.0).await?;
    let total_count = ids.len() as i32;
    let page_ids: Vec<uuid::Uuid> = ids.into_iter().skip(offset).take(limit).collect();

    // Defence in depth: the id list is already account-scoped, but the row
    // fetch re-asserts it so this resolver cannot leak on its own.
    let rows = if page_ids.is_empty() || scoped.is_empty() {
        Vec::new()
    } else {
        transaction::Entity::find()
            .filter(transaction::Column::Id.is_in(page_ids.clone()))
            .filter(transaction::Column::AccountId.is_in(scoped))
            .all(db)
            .await?
    };
    let mut by_id: HashMap<uuid::Uuid, transaction::Model> =
        rows.into_iter().map(|r| (r.id, r)).collect();
    // Keep the engine's newest-first order.
    let items = page_ids
        .iter()
        .filter_map(|id| by_id.remove(id))
        .map(to_graphql_transaction)
        .collect();

    Ok(TransactionPage {
        items,
        total_count,
        limit: limit as i32,
        offset: offset as i32,
    })
}

// ---------------------------------------------------------------------------
// Mutations (publish-then-upsert)
// ---------------------------------------------------------------------------

/// Revision-guarded upsert of the `goal` projection, identical in effect to
/// `projection::goals::project_goal` so the caller reads its own write back
/// without waiting on the projector. Never rewrites `owner_user_id`.
async fn upsert_goal(
    db: &DatabaseConnection,
    record: &wire::GoalRecord,
) -> async_graphql::Result<()> {
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        r#"INSERT INTO goal
             (id, owner_user_id, name, goal_type, amount, currency,
              scope_category_slugs, scope_tags, scope_combine, scope_tag_combine,
              period_kind, period_cadence, period_start, period_end, archived, revision)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
           ON CONFLICT (id) DO UPDATE SET
             name = excluded.name, goal_type = excluded.goal_type,
             amount = excluded.amount, currency = excluded.currency,
             scope_category_slugs = excluded.scope_category_slugs,
             scope_tags = excluded.scope_tags,
             scope_combine = excluded.scope_combine,
             scope_tag_combine = excluded.scope_tag_combine,
             period_kind = excluded.period_kind, period_cadence = excluded.period_cadence,
             period_start = excluded.period_start, period_end = excluded.period_end,
             archived = excluded.archived, revision = excluded.revision
           WHERE goal.revision <= excluded.revision"#,
        vec![
            record.id.into(),
            record.owner_user_id.into(),
            record.name.clone().into(),
            type_str(record.goal_type).into(),
            record.amount.into(),
            record.currency.clone().into(),
            record.scope.category_slugs.clone().into(),
            record.scope.tags.clone().into(),
            combine_str(record.scope.combine).into(),
            combine_str(record.scope.tag_combine).into(),
            kind_str(record.period.kind).into(),
            record
                .period
                .cadence
                .map(|c| cadence_str(c).to_string())
                .into(),
            record.period.start_date.into(),
            record.period.end_date.into(),
            record.archived.into(),
            record.revision.into(),
        ],
    );
    db.execute(stmt).await?;
    Ok(())
}

async fn publish_and_project(
    db: &DatabaseConnection,
    publisher: &Arc<EventPublisher>,
    record: &wire::GoalRecord,
) -> async_graphql::Result<()> {
    let value = serde_json::to_vec(record)
        .map_err(|e| async_graphql::Error::new(format!("failed to serialize goal: {e}")))?;
    publish_event(publisher, wire::TOPIC_GOAL, &record.id.to_string(), &value).await?;
    upsert_goal(db, record).await
}

fn record_from(
    id: uuid::Uuid,
    owner_user_id: uuid::Uuid,
    archived: bool,
    v: ValidInput,
) -> wire::GoalRecord {
    wire::GoalRecord {
        schema_version: wire::CURRENT_SCHEMA_VERSION,
        id,
        owner_user_id,
        name: v.name,
        goal_type: v.goal_type,
        amount: v.amount,
        currency: v.currency,
        scope: v.scope,
        period: v.period,
        archived,
        revision: Utc::now(),
    }
}

/// The complete record for an already-projected goal.
fn record_of(existing: goal::Model) -> wire::GoalRecord {
    wire::GoalRecord {
        schema_version: wire::CURRENT_SCHEMA_VERSION,
        id: existing.id,
        owner_user_id: existing.owner_user_id,
        name: existing.name,
        goal_type: parse_type(&existing.goal_type),
        amount: existing.amount,
        currency: existing.currency,
        scope: wire::GoalScope {
            category_slugs: existing.scope_category_slugs,
            tags: existing.scope_tags,
            combine: parse_combine(&existing.scope_combine),
            tag_combine: parse_combine(&existing.scope_tag_combine),
        },
        period: wire::GoalPeriod {
            kind: parse_kind(&existing.period_kind),
            cadence: existing.period_cadence.as_deref().and_then(parse_cadence),
            start_date: existing.period_start,
            end_date: existing.period_end,
        },
        archived: existing.archived,
        revision: Utc::now(),
    }
}

async fn reload(db: &DatabaseConnection, id: uuid::Uuid) -> async_graphql::Result<Goal> {
    let row = goal::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| async_graphql::Error::new("goal upsert did not take effect"))?;
    render(db, row).await
}

pub async fn create_goal(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    max_tags: u32,
    input: GoalInput,
) -> async_graphql::Result<Goal> {
    let valid = validate_input(&input, max_tags)?;
    check_categories_exist(db, &valid.scope.category_slugs).await?;
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    let id = uuid::Uuid::new_v4();
    let record = record_from(id, user.user_id, false, valid);
    publish_and_project(db, publisher, &record).await?;
    reload(db, id).await
}

/// Read-modify-write (§2.1): the topic is whole-state, so the record is
/// rebuilt from the projected goal plus the change. What the input does not
/// carry — id, owner, `archived` — comes from the loaded goal.
pub async fn update_goal(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    max_tags: u32,
    id: Uuid,
    input: GoalInput,
) -> async_graphql::Result<Goal> {
    let existing = require_owned(db, user, id.0).await?;
    let valid = validate_input(&input, max_tags)?;
    check_categories_exist(db, &valid.scope.category_slugs).await?;
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    let record = record_from(existing.id, existing.owner_user_id, existing.archived, valid);
    publish_and_project(db, publisher, &record).await?;
    reload(db, existing.id).await
}

/// Sets `archived` on the whole record; the goal is not tombstoned, so it
/// stays readable with `includeArchived`.
pub async fn archive_goal(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    id: Uuid,
) -> async_graphql::Result<Goal> {
    let existing = require_owned(db, user, id.0).await?;
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    let mut record = record_of(existing);
    record.archived = true;
    publish_and_project(db, publisher, &record).await?;
    reload(db, record.id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dec(s: &str) -> RustDecimal {
        s.parse().unwrap()
    }

    fn d(y: i32, m: u32, day: u32) -> Date {
        Date(NaiveDate::from_ymd_opt(y, m, day).unwrap())
    }

    fn input() -> GoalInput {
        GoalInput {
            name: "Hobbies".into(),
            goal_type: GoalType::SpendingLimit,
            amount: Decimal(dec("200")),
            currency: "EUR".into(),
            category_slugs: vec!["leisure.hobbies".into()],
            tags: vec![],
            combine: ScopeCombine::All,
            tag_combine: ScopeCombine::All,
            period_kind: GoalPeriodKind::Recurring,
            cadence: Some(GoalCadence::Monthly),
            start_date: None,
            end_date: None,
        }
    }

    fn rejected(i: GoalInput) {
        let err = validate_input(&i, 10).expect_err("should be rejected");
        let code = err
            .extensions
            .as_ref()
            .and_then(|e| e.get("code"))
            .map(|v| v.to_string());
        assert_eq!(code.as_deref(), Some("\"VALIDATION\""), "{}", err.message);
    }

    #[test]
    fn limit_is_met_at_or_under_the_threshold() {
        let t = GoalType::SpendingLimit;
        assert!(is_met(t, dec("200"), dec("199.99")));
        assert!(is_met(t, dec("200"), dec("200")), "exactly at the limit is met");
        assert!(!is_met(t, dec("200"), dec("200.01")));
        assert!(is_met(t, dec("200"), dec("0")));
    }

    #[test]
    fn target_is_met_at_or_over_the_threshold() {
        let t = GoalType::SavingTarget;
        assert!(!is_met(t, dec("500"), dec("499.99")));
        assert!(is_met(t, dec("500"), dec("500")), "exactly at the target is met");
        assert!(is_met(t, dec("500"), dec("500.01")));
        assert!(!is_met(t, dec("500"), dec("0")));
    }

    #[test]
    fn threshold_comparison_ignores_decimal_scale() {
        assert!(is_met(GoalType::SpendingLimit, dec("200.0000"), dec("200")));
        assert!(is_met(GoalType::SavingTarget, dec("200"), dec("200.0000")));
    }

    #[test]
    fn remaining_is_amount_minus_total_and_negative_when_over() {
        assert_eq!(remaining(dec("200"), dec("150.50")), dec("49.50"));
        assert_eq!(remaining(dec("200"), dec("200")), dec("0"));
        assert_eq!(remaining(dec("200"), dec("230")), dec("-30"));
    }

    #[test]
    fn bucket_mapping_derives_remaining_and_met() {
        let b = BucketProgress {
            start: d(2027, 1, 1).0,
            end: d(2027, 1, 31).0,
            label: "Jan 2027".into(),
            total: dec("200"),
            pending: dec("12"),
            in_progress: false,
        };
        let out = to_bucket(GoalType::SpendingLimit, dec("200"), b);
        assert_eq!(out.remaining.0, dec("0"));
        assert!(out.met);
        assert_eq!(out.pending.0, dec("12"));
    }

    #[test]
    fn a_valid_recurring_input_normalises_fields() {
        let mut i = input();
        i.name = "  Hobbies ".into();
        i.currency = "eur".into();
        i.tags = vec!["Hobby Time".into(), "hobby-time".into()];
        let v = validate_input(&i, 10).unwrap();
        assert_eq!(v.name, "Hobbies");
        assert_eq!(v.currency, "EUR");
        assert_eq!(v.scope.tags, vec!["hobby-time"]);
        assert_eq!(v.period.kind, wire::PeriodKind::Recurring);
    }

    #[test]
    fn rejects_non_positive_amount() {
        for amount in ["0", "-5", "0.00001"] {
            let mut i = input();
            i.amount = Decimal(dec(amount));
            rejected(i);
        }
    }

    #[test]
    fn rejects_an_empty_scope() {
        let mut i = input();
        i.category_slugs = vec![];
        rejected(i);
        let mut i = input();
        i.category_slugs = vec!["  ".into()];
        i.tags = vec!["!!!".into()];
        rejected(i);
    }

    #[test]
    fn a_tags_only_scope_is_valid() {
        let mut i = input();
        i.category_slugs = vec![];
        i.tags = vec!["hobby".into()];
        assert!(validate_input(&i, 10).is_ok());
    }

    #[test]
    fn rejects_blank_name_and_currency() {
        let mut i = input();
        i.name = " ".into();
        rejected(i);
        let mut i = input();
        i.currency = "".into();
        rejected(i);
    }

    #[test]
    fn recurring_requires_cadence_and_rejects_dates() {
        let mut i = input();
        i.cadence = None;
        rejected(i);
        let mut i = input();
        i.start_date = Some(d(2027, 1, 1));
        rejected(i);
        let mut i = input();
        i.end_date = Some(d(2027, 1, 1));
        rejected(i);
    }

    #[test]
    fn fixed_requires_start_and_rejects_cadence() {
        let mut i = input();
        i.period_kind = GoalPeriodKind::Fixed;
        i.cadence = None;
        i.start_date = None;
        rejected(i);
        let mut i = input();
        i.period_kind = GoalPeriodKind::Fixed;
        i.start_date = Some(d(2027, 1, 1));
        // cadence still set
        rejected(i);
    }

    #[test]
    fn fixed_end_must_not_precede_start_but_may_equal_it_or_be_open() {
        let fixed = |end: Option<Date>| {
            let mut i = input();
            i.period_kind = GoalPeriodKind::Fixed;
            i.cadence = None;
            i.start_date = Some(d(2027, 3, 1));
            i.end_date = end;
            i
        };
        rejected(fixed(Some(d(2027, 2, 28))));
        assert!(validate_input(&fixed(Some(d(2027, 3, 1))), 10).is_ok());
        assert!(validate_input(&fixed(None), 10).is_ok());
    }

    #[test]
    fn too_many_tags_is_rejected() {
        let mut i = input();
        i.tags = vec!["a".into(), "b".into(), "c".into()];
        assert!(validate_input(&i, 2).is_err());
    }

    #[test]
    fn window_needs_both_bounds_or_neither_and_in_order() {
        assert!(parse_window(None, None).unwrap().is_none());
        let w = parse_window(Some(d(2027, 1, 1)), Some(d(2027, 1, 1)))
            .unwrap()
            .unwrap();
        assert_eq!(w.start, w.end);
        assert!(parse_window(Some(d(2027, 2, 1)), Some(d(2027, 1, 1))).is_err());
        assert!(parse_window(Some(d(2027, 1, 1)), None).is_err());
        assert!(parse_window(None, Some(d(2027, 1, 1))).is_err());
    }
}
