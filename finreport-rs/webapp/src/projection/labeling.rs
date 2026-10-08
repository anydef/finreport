//! Projections for the five new §2.2 labeling topics
//! (`transaction-label`, `llm-cache`, `user-label`, `rule`, `category`) into
//! the §3 tables (`transaction_label`, `llm_label_cache`,
//! `transaction_user_label`/`transaction_split`, `rule`, `category`).
//!
//! **Owned by WP3.** Unlike the three original ingest topics, none of these
//! five tables carries a foreign key (§3 "No foreign keys on these five
//! tables"): every reference is a **deterministic UUIDv5** computed straight
//! from the referenced entity's natural key (`category_uuid`, `transaction_uuid`,
//! `learned_rule_uuid`, `split_uuid` — all frozen by WP0 in `kafka::envelope`/
//! `kafka::labeling`), so resolving a `category_slug` into the `category_id`
//! column this table actually stores never needs a lookup query that could
//! race a still-in-flight batch.
//!
//! `category`, `transaction_user_label` and `rule` carry a `revision` and are
//! guarded by the §2.1 last-writer-wins rule
//! (`WHERE excluded.revision >= row.revision`, enforced here via a raw
//! `excluded.*` reference in the `ON CONFLICT ... DO UPDATE` clause, the same
//! idiom `projection::upsert::legacy_guard` uses for the origin guard).
//! `transaction_label`/`llm_label_cache` carry no `revision` field (§2.2) —
//! they are the labeler's own exclusive output, applied in the order the
//! labeler itself decided to publish them, so a plain overwrite is correct.
//!
//! None of this is wired into the live `projector` consumer loop
//! (`finreport.category`/`.rule`/`.user-label` have no other writer than the
//! labeler's own consume loop below, plus `category-seed`'s and GraphQL
//! mutations' own dual-write, per §2.1); `webapp::labeling::processor` (also
//! WP3) is the only caller besides `category-seed`.

use entity::entities::{
    category, llm_label_cache, rule, transaction, transaction_label, transaction_split,
    transaction_user_label,
};
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect,
};
use tracing::warn;
use uuid::Uuid;

use crate::kafka::labeling::{
    category_uuid, split_uuid, CacheRecord, CategoryKind, CategoryOrigin, CategoryRecord,
    LabelRecord, LabelSource, LabelStatus, ReviewReason, RuleOrigin, RuleRecord, RuleState,
    UserLabelRecord, CURRENT_SCHEMA_VERSION,
};

fn category_kind_str(kind: CategoryKind) -> &'static str {
    match kind {
        CategoryKind::Income => "income",
        CategoryKind::Expense => "expense",
        CategoryKind::Transfer => "transfer",
        CategoryKind::Saving => "saving",
    }
}

fn category_origin_str(origin: CategoryOrigin) -> &'static str {
    match origin {
        CategoryOrigin::Seed => "seed",
        CategoryOrigin::User => "user",
    }
}

fn rule_state_str(state: RuleState) -> &'static str {
    match state {
        RuleState::Active => "active",
        RuleState::InReview => "in_review",
        RuleState::Revoked => "revoked",
        RuleState::Rejected => "rejected",
    }
}

fn rule_origin_str(origin: RuleOrigin) -> &'static str {
    match origin {
        RuleOrigin::User => "user",
        RuleOrigin::Learned => "learned",
    }
}

pub(crate) fn label_source_str(source: LabelSource) -> &'static str {
    match source {
        LabelSource::User => "user",
        LabelSource::Rule => "rule",
        LabelSource::LlmCache => "llm-cache",
        LabelSource::Llm => "llm",
    }
}

pub(crate) fn label_status_str(status: LabelStatus) -> &'static str {
    match status {
        LabelStatus::Resolved => "resolved",
        LabelStatus::NeedsReview => "needs_review",
    }
}

pub(crate) fn review_reason_str(reason: ReviewReason) -> &'static str {
    match reason {
        ReviewReason::Ambiguous => "ambiguous",
        ReviewReason::NewCategory => "new_category",
        ReviewReason::ProviderError => "provider_error",
        ReviewReason::SplitMismatch => "split_mismatch",
    }
}

/// `f32` confidence (§2.4/§2.9, clamped `0.0..=1.0` at the provider boundary)
/// to the `NUMERIC(4,3)` column every labeling table stores it as.
fn confidence_decimal(confidence: f32) -> Decimal {
    Decimal::from_f32(confidence.clamp(0.0, 1.0)).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// §3 category
// ---------------------------------------------------------------------------

/// Upserts (or, on `None`, tombstone-deletes) one `category` row.
///
/// `parent_id`/`id` are **not** looked up — they are the deterministic
/// `category_uuid(slug)` WP0 froze, computed straight from the slug, so a
/// child can project before its parent exists (§3) and the link still
/// resolves once the parent lands.
pub async fn project_category(
    txn: &impl ConnectionTrait,
    category_id: Uuid,
    record: Option<CategoryRecord>,
) -> Result<(), DbErr> {
    let Some(record) = record else {
        category::Entity::delete_by_id(category_id).exec(txn).await?;
        return Ok(());
    };

    let parent_id = record.parent_slug.as_deref().map(category_uuid);
    let model = category::ActiveModel {
        id: Set(category_id),
        slug: Set(record.slug.clone()),
        parent_id: Set(parent_id),
        name: Set(record.name.clone()),
        kind: Set(category_kind_str(record.kind).to_string()),
        depth: Set(record.depth),
        sort_order: Set(record.sort_order),
        archived: Set(record.archived),
        origin: Set(category_origin_str(record.origin).to_string()),
        owner_user_id: Set(record.owner_user_id),
        revision: Set(record.revision.into()),
    };

    let mut on_conflict = sea_orm::sea_query::OnConflict::column(category::Column::Id);
    on_conflict
        .update_columns([
            category::Column::Slug,
            category::Column::ParentId,
            category::Column::Name,
            category::Column::Kind,
            category::Column::Depth,
            category::Column::SortOrder,
            category::Column::Archived,
            category::Column::Origin,
            category::Column::OwnerUserId,
            category::Column::Revision,
        ])
        .action_cond_where(
            Expr::col((category::Entity, category::Column::Revision))
                .lte(Expr::cust("excluded.revision")),
        );

    category::Entity::insert(model)
        .on_conflict(on_conflict.to_owned())
        .exec(txn)
        .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// §2.4 transaction_label / llm_label_cache — the labeler's own exclusive
// output, no `revision` field, applied in publish order (plain overwrite).
// ---------------------------------------------------------------------------

/// Upserts (or tombstone-deletes) one `transaction_label` row.
///
/// `category_id`/`rule_id` are stored verbatim from the record (already
/// deterministic UUIDs, §2.4) — no lookup. Compare-before-publish (§2.3) is
/// the *caller's* responsibility (`labeling::processor`): by the time a
/// `LabelRecord` reaches this function it has already been decided worth
/// publishing.
pub async fn project_transaction_label(
    txn: &impl ConnectionTrait,
    transaction_id: Uuid,
    record: Option<LabelRecord>,
) -> Result<(), DbErr> {
    let Some(record) = record else {
        transaction_label::Entity::delete_by_id(transaction_id)
            .exec(txn)
            .await?;
        return Ok(());
    };

    let model = transaction_label::ActiveModel {
        transaction_id: Set(transaction_id),
        category_id: Set(record.category_slug.as_deref().map(category_uuid)),
        label_source: Set(label_source_str(record.label_source).to_string()),
        rule_id: Set(record.rule_id),
        confidence: Set(record.confidence.map(confidence_decimal)),
        status: Set(label_status_str(record.status).to_string()),
        review_reason: Set(record.review_reason.map(|r| review_reason_str(r).to_string())),
        proposed_category_path: Set(record.proposed_category_path.clone()),
        provider: Set(record.provider.clone()),
        model: Set(record.model.clone()),
        prompt_version: Set(record.prompt_version.clone()),
        fingerprint: Set(record.fingerprint.clone()),
        reasoning: Set(record.reasoning.clone()),
        labeled_at: Set(record.labeled_at.into()),
    };

    transaction_label::Entity::insert(model)
        .on_conflict(
            sea_orm::sea_query::OnConflict::column(transaction_label::Column::TransactionId)
                .update_columns([
                    transaction_label::Column::CategoryId,
                    transaction_label::Column::LabelSource,
                    transaction_label::Column::RuleId,
                    transaction_label::Column::Confidence,
                    transaction_label::Column::Status,
                    transaction_label::Column::ReviewReason,
                    transaction_label::Column::ProposedCategoryPath,
                    transaction_label::Column::Provider,
                    transaction_label::Column::Model,
                    transaction_label::Column::PromptVersion,
                    transaction_label::Column::Fingerprint,
                    transaction_label::Column::Reasoning,
                    transaction_label::Column::LabeledAt,
                ])
                .to_owned(),
        )
        .exec(txn)
        .await?;

    Ok(())
}

/// Upserts (or tombstone-deletes) one `llm_label_cache` row, keyed by
/// `fingerprint`.
pub async fn project_llm_cache(
    txn: &impl ConnectionTrait,
    fingerprint: &str,
    record: Option<CacheRecord>,
) -> Result<(), DbErr> {
    let Some(record) = record else {
        llm_label_cache::Entity::delete_by_id(fingerprint.to_string())
            .exec(txn)
            .await?;
        return Ok(());
    };

    let model = llm_label_cache::ActiveModel {
        fingerprint: Set(fingerprint.to_string()),
        category_id: Set(record.category_slug.as_deref().map(category_uuid)),
        proposed_path: Set(record.proposed_path.clone()),
        confidence: Set(confidence_decimal(record.confidence)),
        provider: Set(record.provider.clone()),
        model: Set(record.model.clone()),
        prompt_version: Set(record.prompt_version.clone()),
        reasoning: Set(record.reasoning.clone()),
        created_at: Set(record.created_at.into()),
    };

    llm_label_cache::Entity::insert(model)
        .on_conflict(
            sea_orm::sea_query::OnConflict::column(llm_label_cache::Column::Fingerprint)
                .update_columns([
                    llm_label_cache::Column::CategoryId,
                    llm_label_cache::Column::ProposedPath,
                    llm_label_cache::Column::Confidence,
                    llm_label_cache::Column::Provider,
                    llm_label_cache::Column::Model,
                    llm_label_cache::Column::PromptVersion,
                    llm_label_cache::Column::Reasoning,
                    llm_label_cache::Column::CreatedAt,
                ])
                .to_owned(),
        )
        .exec(txn)
        .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// §2.6 user-label (override + splits)
// ---------------------------------------------------------------------------

/// Upserts (or tombstone-deletes) the `transaction_user_label` row and its
/// `transaction_split` rows for one transaction (§2.6).
///
/// A `Some(record)` with `category_slug: None` and empty `parts` is the
/// "clear the override" event (§2.6) — distinct from `None` ("the
/// transaction is gone"): both delete the current splits, but only `None`
/// deletes the `transaction_user_label` row itself.
pub async fn project_user_label(
    txn: &impl ConnectionTrait,
    transaction_id: Uuid,
    record: Option<UserLabelRecord>,
) -> Result<(), DbErr> {
    let Some(record) = record else {
        transaction_split::Entity::delete_many()
            .filter(transaction_split::Column::TransactionId.eq(transaction_id))
            .exec(txn)
            .await?;
        transaction_user_label::Entity::delete_by_id(transaction_id)
            .exec(txn)
            .await?;
        return Ok(());
    };

    // Last-writer-wins (§2.1): skip entirely if a newer revision is already
    // stored, rather than letting an out-of-order replay message win.
    if let Some(existing) = transaction_user_label::Entity::find_by_id(transaction_id)
        .one(txn)
        .await?
        && existing.revision > record.revision
    {
        return Ok(());
    }

    let model = transaction_user_label::ActiveModel {
        transaction_id: Set(transaction_id),
        category_id: Set(record.category_slug.as_deref().map(category_uuid)),
        note: Set(record.note.clone()),
        // Iteration 3 §2.1: whole-state record, so every mutation republishes
        // `recurring` alongside the pre-existing fields.
        recurring: Set(record.recurring),
        revision: Set(record.revision.into()),
    };

    transaction_user_label::Entity::insert(model)
        .on_conflict(
            sea_orm::sea_query::OnConflict::column(transaction_user_label::Column::TransactionId)
                .update_columns([
                    transaction_user_label::Column::CategoryId,
                    transaction_user_label::Column::Note,
                    transaction_user_label::Column::Recurring,
                    transaction_user_label::Column::Revision,
                ])
                .to_owned(),
        )
        .exec(txn)
        .await?;

    // Splits are replaced wholesale: delete-then-reinsert is simpler than a
    // per-index diff, and splits are never numerous enough for that to
    // matter (§2.6 bounds a split's shape, not its count).
    transaction_split::Entity::delete_many()
        .filter(transaction_split::Column::TransactionId.eq(transaction_id))
        .exec(txn)
        .await?;

    for part in &record.parts {
        let id = split_uuid(transaction_id, part.index);
        let model = transaction_split::ActiveModel {
            id: Set(id),
            transaction_id: Set(transaction_id),
            part_index: Set(part.index),
            amount: Set(part.amount),
            category_id: Set(category_uuid(&part.category_slug)),
            invalid: Set(false),
        };
        transaction_split::Entity::insert(model).exec(txn).await?;
    }

    Ok(())
}

/// §2.5 edge case: a `NOTBOOKED` transaction re-published booked with a
/// different amount. If this transaction has valid splits that no longer sum
/// to `new_amount`, marks them `invalid = true` (never silently rescaled) and
/// reports that a `split_mismatch` review is now needed. A transaction with
/// no splits, or whose splits already matched and still do, is untouched.
pub async fn invalidate_mismatched_splits(
    txn: &impl ConnectionTrait,
    transaction_id: Uuid,
    new_amount: Decimal,
) -> Result<bool, DbErr> {
    let splits = transaction_split::Entity::find()
        .filter(transaction_split::Column::TransactionId.eq(transaction_id))
        .filter(transaction_split::Column::Invalid.eq(false))
        .all(txn)
        .await?;

    if splits.is_empty() {
        return Ok(false);
    }

    let sum: Decimal = splits.iter().map(|s| s.amount).sum();
    if sum == new_amount {
        return Ok(false);
    }

    transaction_split::Entity::update_many()
        .col_expr(transaction_split::Column::Invalid, Expr::value(true))
        .filter(transaction_split::Column::TransactionId.eq(transaction_id))
        .exec(txn)
        .await?;

    warn!(
        %transaction_id, %new_amount, %sum,
        "labeler: split no longer sums to the (re)booked amount, marked invalid"
    );
    Ok(true)
}

// ---------------------------------------------------------------------------
// §2.7/§2.8 rule
// ---------------------------------------------------------------------------

/// Upserts (or tombstone-deletes) one `rule` row.
pub async fn project_rule(
    txn: &impl ConnectionTrait,
    rule_id: Uuid,
    record: Option<RuleRecord>,
) -> Result<(), DbErr> {
    let Some(record) = record else {
        rule::Entity::delete_by_id(rule_id).exec(txn).await?;
        return Ok(());
    };

    let conditions_json =
        serde_json::to_value(&record.conditions).unwrap_or(serde_json::Value::Null);
    let model = rule::ActiveModel {
        id: Set(rule_id),
        name: Set(record.name.clone()),
        category_id: Set(category_uuid(&record.category_slug)),
        conditions: Set(conditions_json),
        priority: Set(record.priority),
        state: Set(rule_state_str(record.state).to_string()),
        origin: Set(rule_origin_str(record.origin).to_string()),
        auto_approved: Set(record.auto_approved),
        user_touched: Set(record.user_touched),
        confidence: Set(record.confidence.map(confidence_decimal)),
        evidence: Set(record.evidence.clone()),
        created_at: Set(record.created_at.into()),
        revision: Set(record.revision.into()),
    };

    let mut on_conflict = sea_orm::sea_query::OnConflict::column(rule::Column::Id);
    on_conflict
        .update_columns([
            rule::Column::Name,
            rule::Column::CategoryId,
            rule::Column::Conditions,
            rule::Column::Priority,
            rule::Column::State,
            rule::Column::Origin,
            rule::Column::AutoApproved,
            rule::Column::UserTouched,
            rule::Column::Confidence,
            rule::Column::Evidence,
            rule::Column::CreatedAt,
            rule::Column::Revision,
        ])
        .action_cond_where(Expr::col((rule::Entity, rule::Column::Revision)).lte(Expr::cust("excluded.revision")));

    match rule::Entity::insert(model).on_conflict(on_conflict.to_owned()).exec(txn).await {
        Ok(_) => Ok(()),
        // A real conflict whose stored revision is already newer: the
        // conditional `WHERE` legitimately declines the update, and
        // sea-orm's `RETURNING`-based insert surfaces that as an error
        // rather than "0 rows, nothing to do" — same last-writer-wins
        // semantics as `project_user_label`'s explicit pre-check (§2.1),
        // just enforced in SQL here. Seen in practice when the labeler's
        // own re-affirming calls to `maybe_learn_rule` (each bumping the
        // revision) echo back out of order through its own consumption of
        // `finreport.rule` (one of its `LABELER_INPUT_TOPICS`).
        Err(DbErr::RecordNotInserted) => Ok(()),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Query helpers `labeling::processor` needs against the projection. Kept
// here (not in `processor.rs`) because they are schema-shaped, like the
// `project_*` functions above, not loop-control logic.
// ---------------------------------------------------------------------------

/// Active rules, for `rules::most_specific_match` (§2.7) to pick from.
pub async fn active_rules(db: &impl ConnectionTrait) -> Result<Vec<RuleRecord>, DbErr> {
    let rows = rule::Entity::find()
        .filter(rule::Column::State.eq("active"))
        .all(db)
        .await?;
    Ok(rows.into_iter().filter_map(rule_model_to_record).collect())
}

/// The rule with this id, as a [`RuleRecord`] — used by the processor to
/// check `user_touched`/`state` before letting the learner touch it (§2.8).
pub async fn find_rule(
    db: &impl ConnectionTrait,
    rule_id: Uuid,
) -> Result<Option<RuleRecord>, DbErr> {
    Ok(rule::Entity::find_by_id(rule_id)
        .one(db)
        .await?
        .and_then(rule_model_to_record))
}

/// `rules::most_specific_match` (§2.7, WP2) only ever reads the `conditions`/
/// `priority`/`id` fields of a [`RuleRecord`] — `category_slug`'s *content*
/// is irrelevant to matching, only to what the caller does with a match
/// afterwards. The projection stores `category_id`, not the slug the wire
/// record carried, so rather than a DB join on every call this encodes the
/// id as a string here; [`category_id_of`] decodes it back, and the real
/// slug (needed only once a rule actually wins, to publish a readable
/// `LabelRecord`) is resolved separately via [`find_category_slug`].
fn rule_model_to_record(row: rule::Model) -> Option<RuleRecord> {
    let state = match row.state.as_str() {
        "active" => RuleState::Active,
        "in_review" => RuleState::InReview,
        "revoked" => RuleState::Revoked,
        "rejected" => RuleState::Rejected,
        other => {
            warn!(state = other, rule_id = %row.id, "labeling: unknown rule.state, skipped");
            return None;
        }
    };
    let origin = match row.origin.as_str() {
        "user" => RuleOrigin::User,
        "learned" => RuleOrigin::Learned,
        other => {
            warn!(origin = other, rule_id = %row.id, "labeling: unknown rule.origin, skipped");
            return None;
        }
    };
    let conditions = serde_json::from_value(row.conditions).unwrap_or_default();
    Some(RuleRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        id: row.id,
        name: row.name,
        category_slug: row.category_id.to_string(),
        conditions,
        priority: row.priority,
        state,
        origin,
        auto_approved: row.auto_approved,
        user_touched: row.user_touched,
        confidence: row.confidence.and_then(|d| d.to_string().parse().ok()),
        evidence: row.evidence,
        created_at: row.created_at.into(),
        revision: row.revision.into(),
    })
}

/// Decodes the `category_id` a [`rule_model_to_record`]-produced
/// [`RuleRecord`] stashed in `category_slug` back into a real `Uuid`. `None`
/// (not a panic) for a `RuleRecord` built by hand with a real slug instead
/// (e.g. a unit test's fake data).
pub fn category_id_of(rule: &RuleRecord) -> Option<Uuid> {
    Uuid::parse_str(&rule.category_slug).ok()
}

/// The real `slug` for a category id, looked up once at publish time — the
/// one place a `transaction_label`/`llm-cache` wire record's `category_slug`
/// field needs a readable name instead of the id the projection stores it as.
pub async fn find_category_slug(
    db: &impl ConnectionTrait,
    category_id: Uuid,
) -> Result<Option<String>, DbErr> {
    Ok(category::Entity::find_by_id(category_id)
        .one(db)
        .await?
        .map(|c| c.slug))
}

/// The seeded category catalog (§3, at most 3 levels deep), for the LLM
/// provider's `LabelRequest::catalog` (§2.9) — slug/name/kind only, never
/// parent/child structure.
pub async fn build_catalog(
    db: &impl ConnectionTrait,
) -> Result<categorizer::provider::CategoryCatalog, DbErr> {
    use categorizer::provider::{CatalogEntry, CategoryCatalog};

    let rows = category::Entity::find()
        .filter(category::Column::Archived.eq(false))
        .all(db)
        .await?;
    Ok(CategoryCatalog::new(
        rows.into_iter()
            .map(|c| CatalogEntry {
                slug: c.slug,
                name: c.name,
                kind: c.kind,
            })
            .collect(),
    ))
}

/// Every `transaction_id` currently labelled by `rule_id` (§2.3: "a rule
/// record resolves every transaction the rule matches now **or** matched
/// before the edit").
pub async fn transaction_ids_labelled_by_rule(
    db: &impl ConnectionTrait,
    rule_id: Uuid,
) -> Result<Vec<Uuid>, DbErr> {
    let rows = transaction_label::Entity::find()
        .filter(transaction_label::Column::RuleId.eq(rule_id))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| r.transaction_id).collect())
}

/// The stored label for one transaction, if any — what
/// `labeling::processor`'s compare-before-publish rule (§2.3) diffs the
/// freshly resolved outcome against.
pub async fn find_transaction_label(
    db: &impl ConnectionTrait,
    transaction_id: Uuid,
) -> Result<Option<transaction_label::Model>, DbErr> {
    transaction_label::Entity::find_by_id(transaction_id).one(db).await
}

/// The stored cache entry for one fingerprint (§2.5 step 4).
pub async fn find_cache(
    db: &impl ConnectionTrait,
    fingerprint: &str,
) -> Result<Option<llm_label_cache::Model>, DbErr> {
    llm_label_cache::Entity::find_by_id(fingerprint.to_string())
        .one(db)
        .await
}

/// The user override/splits for one transaction, if any (§2.5 steps 1-2).
pub async fn find_user_label(
    db: &impl ConnectionTrait,
    transaction_id: Uuid,
) -> Result<Option<transaction_user_label::Model>, DbErr> {
    transaction_user_label::Entity::find_by_id(transaction_id)
        .one(db)
        .await
}

/// Valid (`invalid = false`) splits for one transaction, ordered by index —
/// empty when the transaction is not split, or its split was invalidated
/// (§2.5 "split_mismatch" edge case, where totals fall back to the whole
/// transaction until the user fixes it).
pub async fn find_valid_splits(
    db: &impl ConnectionTrait,
    transaction_id: Uuid,
) -> Result<Vec<transaction_split::Model>, DbErr> {
    transaction_split::Entity::find()
        .filter(transaction_split::Column::TransactionId.eq(transaction_id))
        .filter(transaction_split::Column::Invalid.eq(false))
        .order_by_asc(transaction_split::Column::PartIndex)
        .all(db)
        .await
}

/// Sets `transaction.counterparty_key` directly (§2.5, §3): the projector's
/// own `upsert_transaction` (frozen, WP0) always writes the mapper's own
/// `None` for this derived column, so this is a narrow, idempotent `UPDATE`
/// the labeler runs after normalizing, never touching any other column.
pub async fn set_counterparty_key(
    db: &impl ConnectionTrait,
    transaction_id: Uuid,
    counterparty_key: &str,
) -> Result<(), DbErr> {
    transaction::Entity::update_many()
        .col_expr(
            transaction::Column::CounterpartyKey,
            Expr::value(counterparty_key.to_string()),
        )
        .filter(transaction::Column::Id.eq(transaction_id))
        .exec(db)
        .await?;
    Ok(())
}

/// Transactions with no `transaction_label` row, or whose label is
/// `needs_review`/`provider_error` (§2.3 "the sweep"), newest first, bounded
/// by `limit` (the cost guard caps how many of these the caller actually
/// resolves per run, not this query).
pub async fn sweep_candidates(db: &impl ConnectionTrait, limit: u64) -> Result<Vec<Uuid>, DbErr> {
    use sea_orm::sea_query::Query;

    let labelled_ids = Query::select()
        .column(transaction_label::Column::TransactionId)
        .from(transaction_label::Entity)
        .to_owned();

    let unlabelled = transaction::Entity::find()
        .filter(transaction::Column::Id.not_in_subquery(labelled_ids))
        .order_by_desc(transaction::Column::ImportedAt)
        .limit(limit)
        .all(db)
        .await?;

    let mut ids: Vec<Uuid> = unlabelled.into_iter().map(|t| t.id).collect();
    if ids.len() as u64 >= limit {
        return Ok(ids);
    }

    let remaining = limit - ids.len() as u64;
    let held = transaction_label::Entity::find()
        .filter(transaction_label::Column::Status.eq("needs_review"))
        .filter(transaction_label::Column::ReviewReason.eq("provider_error"))
        .order_by_desc(transaction_label::Column::LabeledAt)
        .limit(remaining)
        .all(db)
        .await?;
    ids.extend(held.into_iter().map(|l| l.transaction_id));
    Ok(ids)
}

/// One transaction's fields relevant to resolution/fingerprinting, read back
/// from the projection (as opposed to `records::TransactionRecord`, which
/// only exists transiently while mapping a fresh Kafka record).
pub struct TransactionForLabeling {
    pub id: Uuid,
    pub source: String,
    pub external_id: String,
    pub account_id: Uuid,
    pub amount: Decimal,
    pub currency: String,
    pub counterparty_name: Option<String>,
    pub counterparty_iban: Option<String>,
    pub description: Option<String>,
    pub transaction_type: Option<String>,
    pub counterparty_key: Option<String>,
    pub booking_date: chrono::NaiveDate,
}

/// Same as [`find_transaction`], keyed by `(source, external_id)` — used by
/// the labeler's `transaction`/`user-label`/`label-request` handlers, which
/// only ever have the natural key (a Kafka record key or a wire record's own
/// `source`/`external_id`), never the UUID directly.
pub async fn find_transaction_by_source(
    db: &impl ConnectionTrait,
    source: &str,
    external_id: &str,
) -> Result<Option<TransactionForLabeling>, DbErr> {
    find_transaction(
        db,
        crate::kafka::envelope::transaction_uuid(source, external_id),
    )
    .await
}

/// One label observed for a `counterparty_key`, as the rule learner sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct CounterpartyObservation {
    pub category_slug: String,
    pub confidence: f32,
    /// `true` for a `label_source = 'user'` label (a human decision).
    pub user_confirmed: bool,
}

/// Every observation (category slug, confidence, whether human) labelled `user`/`llm`/`llm-cache`
/// (never `rule` — §2.8 "a rule's own output must not justify itself") for
/// transactions sharing `counterparty_key`, for `labeling::learn::consider`
/// to weigh. A user label's confidence is reported as `1.0` regardless of
/// what is stored (§2.8 "a user-confirmed label counts as 1.0").
pub async fn observations_for_counterparty_key(
    db: &impl ConnectionTrait,
    counterparty_key: &str,
) -> Result<Vec<CounterpartyObservation>, DbErr> {
    let transaction_ids: Vec<Uuid> = transaction::Entity::find()
        .filter(transaction::Column::CounterpartyKey.eq(counterparty_key))
        .all(db)
        .await?
        .into_iter()
        .map(|t| t.id)
        .collect();
    if transaction_ids.is_empty() {
        return Ok(Vec::new());
    }

    let labels = transaction_label::Entity::find()
        .filter(transaction_label::Column::TransactionId.is_in(transaction_ids))
        .filter(transaction_label::Column::LabelSource.is_in(["user", "llm", "llm-cache"]))
        .filter(transaction_label::Column::CategoryId.is_not_null())
        .all(db)
        .await?;

    let category_ids: Vec<Uuid> = labels.iter().filter_map(|l| l.category_id).collect();
    let categories = category::Entity::find()
        .filter(category::Column::Id.is_in(category_ids))
        .all(db)
        .await?;
    let slug_by_id: std::collections::HashMap<Uuid, String> =
        categories.into_iter().map(|c| (c.id, c.slug)).collect();

    Ok(labels
        .into_iter()
        .filter_map(|l| {
            let category_id = l.category_id?;
            let slug = slug_by_id.get(&category_id)?.clone();
            let user_confirmed = l.label_source == "user";
            let confidence = if user_confirmed {
                1.0
            } else {
                l.confidence.and_then(|d| d.to_string().parse().ok()).unwrap_or(0.0)
            };
            Some(CounterpartyObservation { category_slug: slug, confidence, user_confirmed })
        })
        .collect())
}

pub async fn find_transaction(
    db: &impl ConnectionTrait,
    transaction_id: Uuid,
) -> Result<Option<TransactionForLabeling>, DbErr> {
    Ok(transaction::Entity::find_by_id(transaction_id)
        .one(db)
        .await?
        .map(|row| TransactionForLabeling {
            id: row.id,
            source: row.source,
            external_id: row.external_id,
            account_id: row.account_id,
            amount: row.amount,
            currency: row.currency,
            counterparty_name: row.counterparty_name,
            counterparty_iban: row.counterparty_iban,
            description: row.description,
            transaction_type: row.transaction_type,
            counterparty_key: row.counterparty_key,
            booking_date: row.booking_date,
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_decimal_clamps_out_of_range_values() {
        assert_eq!(confidence_decimal(1.5), Decimal::from_f32(1.0).unwrap());
        assert_eq!(confidence_decimal(-0.2), Decimal::from_f32(0.0).unwrap());
        assert_eq!(confidence_decimal(0.42), Decimal::from_f32(0.42).unwrap());
    }

    #[test]
    fn enum_to_db_string_mappings_are_stable() {
        assert_eq!(category_kind_str(CategoryKind::Expense), "expense");
        assert_eq!(category_origin_str(CategoryOrigin::Seed), "seed");
        assert_eq!(rule_state_str(RuleState::InReview), "in_review");
        assert_eq!(rule_origin_str(RuleOrigin::Learned), "learned");
        assert_eq!(label_source_str(LabelSource::LlmCache), "llm-cache");
        assert_eq!(label_status_str(LabelStatus::NeedsReview), "needs_review");
        assert_eq!(review_reason_str(ReviewReason::SplitMismatch), "split_mismatch");
    }
}
