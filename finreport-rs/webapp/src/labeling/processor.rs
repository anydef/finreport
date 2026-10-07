//! §2.3 "the labeler": the four-topic consume → normalize → resolve →
//! compare → publish loop, its own offset rows (`@labeler` suffix), the
//! projector-lag startup guard, the unlabelled sweep and the cost guard.
//!
//! Owned by WP3. Depends on WP0 (this crate's Kafka/entity contracts) and
//! WP2 (`normalize`/`fingerprint`/`rules::most_specific_match`/`learn::consider`).
//! WP2's own `resolve::resolve()` stub takes no parameters (an unusable
//! placeholder, not a real entry point) — the §2.5 precedence chain is
//! instead implemented directly in [`resolve_transaction`] below, calling
//! WP2's four other, independently usable primitives through the injectable
//! [`LabelingOps`] (real implementations in production, fakes in this
//! module's own unit tests, so they never depend on WP2's still-`todo!()`
//! bodies).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use chrono::Utc;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::{BorrowedMessage, Message};
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use rdkafka::ClientConfig;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, TransactionTrait};
use tracing::{error, info, warn};
use uuid::Uuid;

use categorizer::provider::{CategoryCatalog, LabelProvider, LabelRequest};

use crate::kafka::envelope::{Envelope, TOPIC_TRANSACTION};
use crate::kafka::labeling::{
    learned_rule_uuid, CacheRecord, LabelRecord, LabelSource, LabelStatus, LabelRequestRecord,
    LabelRequestTarget, ReviewReason, RuleOrigin, RuleRecord, RuleState, UserLabelRecord,
    CURRENT_SCHEMA_VERSION, TOPIC_LABEL_REQUEST, TOPIC_LLM_CACHE, TOPIC_RULE,
    TOPIC_TRANSACTION_LABEL, TOPIC_USER_LABEL,
};
use crate::kafka::producer::EventPublisher;
use crate::labeling::fingerprint::{self, Direction};
use crate::labeling::learn::{self, LearnedRule, Observation};
use crate::labeling::normalize;
use crate::labeling::rules::{self, RuleMatchInput};
use crate::projection::labeling as proj;
use crate::projection::labeling::TransactionForLabeling;
use crate::projection::offsets;

/// The four topics the labeler consumes (§2.3) — distinct from
/// `projection::INGEST_TOPICS`, which the main `projector` binary owns.
pub const LABELER_INPUT_TOPICS: [&str; 4] = [
    TOPIC_TRANSACTION,
    TOPIC_USER_LABEL,
    TOPIC_RULE,
    TOPIC_LABEL_REQUEST,
];
const LABELER_PARTITION: i32 = 0;
/// Suffix distinguishing the labeler's own `projection_offset` rows from the
/// main projector's, in the same table (§2.3).
pub const LABELER_OFFSET_SUFFIX: &str = "@labeler";

fn offset_topic_key(topic: &str) -> String {
    format!("{topic}{LABELER_OFFSET_SUFFIX}")
}

// ---------------------------------------------------------------------------
// Injectable WP2 primitives (§2.5/§2.7/§2.8)
// ---------------------------------------------------------------------------

/// Wraps WP2's four independently callable primitives (everything but the
/// unusable, parameterless `resolve::resolve()` stub) behind function
/// pointers/closures, so the processor's own unit tests can fake them out
/// instead of hitting WP2's still-`todo!()` bodies. [`LabelingOps::real`]
/// wires the actual WP2 functions for production.
type NormalizeFn = Box<dyn Fn(Option<&str>, Option<&str>) -> String + Send + Sync>;
type FingerprintFn = Box<dyn Fn(&str, &str, &str, &str, &str, Direction) -> String + Send + Sync>;
type MostSpecificMatchFn =
    Box<dyn Fn(&[RuleRecord], &RuleMatchInput<'_>) -> Option<RuleRecord> + Send + Sync>;
type ConsiderFn = Box<dyn for<'a> Fn(&[Observation<'a>]) -> Option<LearnedRule> + Send + Sync>;

pub struct LabelingOps {
    normalize: NormalizeFn,
    fingerprint: FingerprintFn,
    most_specific_match: MostSpecificMatchFn,
    consider: ConsiderFn,
}

impl LabelingOps {
    /// Wires the real WP2 functions, capturing `APP_rule_learn_min_observations`/
    /// `APP_rule_auto_approve_threshold` for `learn::consider`.
    pub fn real(min_observations: u32, auto_approve_threshold: f32) -> Self {
        Self {
            normalize: Box::new(|counterparty, description| {
                normalize::normalize(counterparty, description)
            }),
            fingerprint: Box::new(
                |provider_id, model, prompt_version, counterparty, description, direction| {
                    fingerprint::fingerprint(
                        provider_id,
                        model,
                        prompt_version,
                        counterparty,
                        description,
                        direction,
                    )
                },
            ),
            most_specific_match: Box::new(|rules_slice, input| {
                rules::most_specific_match(rules_slice, input).cloned()
            }),
            consider: Box::new(move |observations| {
                learn::consider(observations, min_observations, auto_approve_threshold)
            }),
        }
    }

    /// Builds a fake set of ops for tests, from plain closures — so a test
    /// can assert on `resolve_transaction`'s orchestration (which step wins,
    /// when a cache hit short-circuits the provider call, ...) without any
    /// of WP2's real (panicking) logic. Not `#[cfg(test)]`-gated: WP3's own
    /// `tests/labeler_postgres.rs` integration suite (a separate crate from
    /// `webapp`'s perspective) needs it too, while WP2's real resolution
    /// logic remains `todo!()`.
    pub fn fake(
        normalize: impl Fn(Option<&str>, Option<&str>) -> String + Send + Sync + 'static,
        fingerprint: impl Fn(&str, &str, &str, &str, &str, Direction) -> String
            + Send
            + Sync
            + 'static,
        most_specific_match: impl Fn(&[RuleRecord], &RuleMatchInput<'_>) -> Option<RuleRecord>
            + Send
            + Sync
            + 'static,
        consider: impl for<'a> Fn(&[Observation<'a>]) -> Option<LearnedRule> + Send + Sync + 'static,
    ) -> Self {
        Self {
            normalize: Box::new(normalize),
            fingerprint: Box::new(fingerprint),
            most_specific_match: Box::new(most_specific_match),
            consider: Box::new(consider),
        }
    }
}

// ---------------------------------------------------------------------------
// §2.3 cost guard
// ---------------------------------------------------------------------------

/// Bounds LLM calls to `APP_llm_max_requests_per_run` for one process run
/// (§2.3). Exhausted ⇒ the labeler leaves the rest unlabelled (not held, not
/// failed) and the next sweep picks them up.
pub struct CostGuard {
    remaining: AtomicU32,
}

impl CostGuard {
    pub fn new(max_requests: u32) -> Self {
        Self { remaining: AtomicU32::new(max_requests) }
    }

    /// Reserves one LLM call, returning `false` (without consuming anything)
    /// once the budget is gone.
    pub fn try_consume(&self) -> bool {
        self.remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                if n == 0 {
                    None
                } else {
                    Some(n - 1)
                }
            })
            .is_ok()
    }

    pub fn is_exhausted(&self) -> bool {
        self.remaining.load(Ordering::SeqCst) == 0
    }
}

// ---------------------------------------------------------------------------
// §2.5 resolution outcome + §2.3 compare-before-publish
// ---------------------------------------------------------------------------

/// What one run of the §2.5 chain decided — the inputs the
/// compare-before-publish rule diffs against the stored row, and what
/// [`build_label_record`] turns into the wire [`LabelRecord`] once the
/// caller has decided to publish.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLabel {
    pub status: LabelStatus,
    pub source: LabelSource,
    pub category_id: Option<Uuid>,
    pub rule_id: Option<Uuid>,
    pub confidence: Option<f32>,
    pub review_reason: Option<ReviewReason>,
    pub proposed_category_path: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    pub fingerprint: Option<String>,
    pub reasoning: Option<String>,
}

/// A stored `transaction_label` row's fields relevant to compare-before-
/// publish — a narrower read than `entity::transaction_label::Model` so
/// [`should_publish`] stays a pure, dependency-free unit test target.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredLabel {
    pub source: LabelSource,
    pub category_id: Option<Uuid>,
    pub rule_id: Option<Uuid>,
    pub status: LabelStatus,
    pub review_reason: Option<ReviewReason>,
    pub proposed_category_path: Option<String>,
    pub fingerprint: Option<String>,
}

/// `llm` and `llm-cache` are the same answer from the labeler's point of
/// view (§2.3): a cache hit reproducing a previous direct LLM call is not a
/// "genuine change" worth republishing.
fn same_source_tier(stored: LabelSource, new: LabelSource) -> bool {
    stored == new
        || matches!(
            (stored, new),
            (LabelSource::Llm, LabelSource::LlmCache) | (LabelSource::LlmCache, LabelSource::Llm)
        )
}

/// §2.3 "compare-before-publish, stated precisely": publishes only when the
/// recomputed outcome is a genuine change (a new override, a rule that now
/// wins or no longer applies, a changed fingerprint, a category that moved),
/// never merely because the textual record would differ (an `llm` label
/// recomputing as `llm-cache` for the same fingerprint and category is *not*
/// a change).
pub fn should_publish(stored: Option<&StoredLabel>, new: &ResolvedLabel) -> bool {
    let Some(stored) = stored else { return true };

    if stored.category_id != new.category_id
        || stored.rule_id != new.rule_id
        || stored.status != new.status
        || stored.review_reason != new.review_reason
        || stored.proposed_category_path != new.proposed_category_path
        || !same_source_tier(stored.source, new.source)
    {
        return true;
    }

    stored.fingerprint != new.fingerprint
}

impl From<&entity::entities::transaction_label::Model> for StoredLabel {
    fn from(row: &entity::entities::transaction_label::Model) -> Self {
        StoredLabel {
            source: parse_label_source(&row.label_source).unwrap_or(LabelSource::User),
            category_id: row.category_id,
            rule_id: row.rule_id,
            status: parse_label_status(&row.status).unwrap_or(LabelStatus::NeedsReview),
            review_reason: row
                .review_reason
                .as_deref()
                .and_then(parse_review_reason),
            proposed_category_path: row.proposed_category_path.clone(),
            fingerprint: row.fingerprint.clone(),
        }
    }
}

fn parse_label_source(value: &str) -> Option<LabelSource> {
    match value {
        "user" => Some(LabelSource::User),
        "rule" => Some(LabelSource::Rule),
        "llm-cache" => Some(LabelSource::LlmCache),
        "llm" => Some(LabelSource::Llm),
        _ => None,
    }
}

fn parse_label_status(value: &str) -> Option<LabelStatus> {
    match value {
        "resolved" => Some(LabelStatus::Resolved),
        "needs_review" => Some(LabelStatus::NeedsReview),
        _ => None,
    }
}

fn parse_review_reason(value: &str) -> Option<ReviewReason> {
    match value {
        "ambiguous" => Some(ReviewReason::Ambiguous),
        "new_category" => Some(ReviewReason::NewCategory),
        "provider_error" => Some(ReviewReason::ProviderError),
        "split_mismatch" => Some(ReviewReason::SplitMismatch),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// §2.5 resolution chain
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ResolveError {
    Db(DbErr),
}

impl From<DbErr> for ResolveError {
    fn from(e: DbErr) -> Self {
        ResolveError::Db(e)
    }
}

fn transaction_direction(amount: rust_decimal::Decimal) -> Direction {
    if amount.is_sign_negative() {
        Direction::Debit
    } else {
        Direction::Credit
    }
}

/// The rules engine's own, coarser direction string (`"SPENDING"`/`"INCOME"`,
/// mirroring the GraphQL `Direction` enum) — distinct from
/// [`transaction_direction`]'s `fingerprint::Direction`.
fn rule_direction_str(amount: rust_decimal::Decimal) -> &'static str {
    if amount.is_sign_negative() {
        "SPENDING"
    } else {
        "INCOME"
    }
}

/// Runs the §2.5 chain for one transaction (split → override → rule →
/// cache → LLM), reading rules/overrides/cache/splits from the projection
/// (`db`) and calling `provider` only on an LLM-cache miss, bounded by
/// `cost_guard`. Returns `Ok(None)` for "nothing to publish yet" (provider
/// error, timeout, or the cost guard is exhausted — §2.5 edge cases: held
/// and unlabelled are different states, so this is distinct from a
/// `needs_review` resolution, which *is* `Some`).
#[allow(clippy::too_many_arguments)]
pub async fn resolve_transaction(
    db: &impl ConnectionTrait,
    ops: &LabelingOps,
    provider: &dyn LabelProvider,
    catalog: &CategoryCatalog,
    prompt_version: &str,
    llm_min_confidence: f32,
    cost_guard: &CostGuard,
    txn: &TransactionForLabeling,
) -> Result<Option<ResolvedLabel>, ResolveError> {
    // 1. Split — no single category; the split itself lives on `user-label`.
    let splits = proj::find_valid_splits(db, txn.id).await?;
    if !splits.is_empty() {
        return Ok(Some(ResolvedLabel {
            status: LabelStatus::Resolved,
            source: LabelSource::User,
            category_id: None,
            rule_id: None,
            confidence: None,
            review_reason: None,
            proposed_category_path: None,
            provider: None,
            model: None,
            prompt_version: None,
            fingerprint: None,
            reasoning: None,
        }));
    }

    // 2. User override.
    if let Some(user_label) = proj::find_user_label(db, txn.id).await?
        && let Some(category_id) = user_label.category_id
    {
        return Ok(Some(ResolvedLabel {
            status: LabelStatus::Resolved,
            source: LabelSource::User,
            category_id: Some(category_id),
            rule_id: None,
            confidence: None,
            review_reason: None,
            proposed_category_path: None,
            provider: None,
            model: None,
            prompt_version: None,
            fingerprint: None,
            reasoning: None,
        }));
    }

    // 3. Rule — the most specific *active* rule whose conditions all match.
    let active_rules = proj::active_rules(db).await?;
    let match_input = RuleMatchInput {
        counterparty_key: txn.counterparty_key.as_deref(),
        counterparty_iban: txn.counterparty_iban.as_deref(),
        description: txn.description.as_deref(),
        transaction_type: txn.transaction_type.as_deref(),
        direction: Some(rule_direction_str(txn.amount)),
        amount: txn.amount,
        account_id: Some(txn.account_id),
    };
    if let Some(matched) = (ops.most_specific_match)(&active_rules, &match_input)
        && let Some(category_id) = proj::category_id_of(&matched)
    {
        return Ok(Some(ResolvedLabel {
            status: LabelStatus::Resolved,
            source: LabelSource::Rule,
            category_id: Some(category_id),
            rule_id: Some(matched.id),
            confidence: None,
            review_reason: None,
            proposed_category_path: None,
            provider: None,
            model: None,
            prompt_version: None,
            fingerprint: None,
            reasoning: None,
        }));
    }

    // 4/5. LLM cache, then the LLM itself.
    let normalized_counterparty = (ops.normalize)(txn.counterparty_name.as_deref(), None);
    let normalized_description = (ops.normalize)(None, txn.description.as_deref());
    let direction = transaction_direction(txn.amount);
    let fp = (ops.fingerprint)(
        provider.id(),
        provider.model(),
        prompt_version,
        &normalized_counterparty,
        &normalized_description,
        direction,
    );

    if let Some(cached) = proj::find_cache(db, &fp).await? {
        return Ok(Some(cache_hit_to_resolution(&cached, llm_min_confidence)));
    }

    if !cost_guard.try_consume() {
        warn!(transaction_id = %txn.id, "labeler: cost guard exhausted, leaving unlabelled for the next sweep");
        return Ok(None);
    }

    let catalog_key = if !normalized_counterparty.is_empty() {
        normalized_counterparty.as_str()
    } else {
        normalized_description.as_str()
    };
    let request = LabelRequest {
        counterparty: Some(catalog_key),
        description: txn.description.as_deref(),
        amount: txn.amount,
        currency: &txn.currency,
        booking_date: txn.booking_date,
        transaction_type: txn.transaction_type.as_deref(),
        catalog,
        prompt_version,
    };

    let suggestion = match provider.suggest(&request).await {
        Ok(suggestion) => suggestion,
        Err(e) => {
            warn!(transaction_id = %txn.id, error = %e, "labeler: provider error, leaving unlabelled for the next sweep");
            return Ok(None);
        }
    };

    let cache_record = CacheRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        fingerprint: fp.clone(),
        category_slug: suggestion.category_slug.clone(),
        proposed_path: suggestion.proposed_path.clone(),
        confidence: suggestion.confidence.clamp(0.0, 1.0),
        provider: provider.id().to_string(),
        model: provider.model().to_string(),
        prompt_version: prompt_version.to_string(),
        reasoning: suggestion.reasoning.clone(),
        created_at: Utc::now(),
    };
    proj::project_llm_cache(db, &fp, Some(cache_record)).await?;

    Ok(Some(suggestion_to_resolution(
        &suggestion,
        provider.id(),
        provider.model(),
        prompt_version,
        &fp,
        llm_min_confidence,
    )))
}

fn cache_hit_to_resolution(
    cached: &entity::entities::llm_label_cache::Model,
    llm_min_confidence: f32,
) -> ResolvedLabel {
    let confidence: f32 = cached.confidence.to_string().parse().unwrap_or(0.0);
    let (status, review_reason) = classify_llm_answer(
        cached.category_id.is_some(),
        cached.proposed_path.is_some(),
        false,
        confidence,
        llm_min_confidence,
    );
    ResolvedLabel {
        status,
        source: LabelSource::LlmCache,
        category_id: cached.category_id,
        rule_id: None,
        confidence: Some(confidence),
        review_reason,
        proposed_category_path: cached.proposed_path.clone(),
        provider: Some(cached.provider.clone()),
        model: Some(cached.model.clone()),
        prompt_version: Some(cached.prompt_version.clone()),
        fingerprint: Some(cached.fingerprint.clone()),
        reasoning: cached.reasoning.clone(),
    }
}

fn suggestion_to_resolution(
    suggestion: &categorizer::provider::LabelSuggestion,
    provider_id: &str,
    model: &str,
    prompt_version: &str,
    fingerprint: &str,
    llm_min_confidence: f32,
) -> ResolvedLabel {
    let (status, review_reason) = classify_llm_answer(
        suggestion.category_slug.is_some(),
        suggestion.proposed_path.is_some(),
        suggestion.ambiguous,
        suggestion.confidence,
        llm_min_confidence,
    );
    ResolvedLabel {
        status,
        source: LabelSource::Llm,
        // Resolved against a slug the *provider* chose from the catalog —
        // turned into a real `category_id` at publish time (deterministic,
        // no DB lookup needed: `category_uuid(slug)`).
        category_id: suggestion
            .category_slug
            .as_deref()
            .map(crate::kafka::labeling::category_uuid),
        rule_id: None,
        confidence: Some(suggestion.confidence.clamp(0.0, 1.0)),
        review_reason,
        proposed_category_path: suggestion.proposed_path.clone(),
        provider: Some(provider_id.to_string()),
        model: Some(model.to_string()),
        prompt_version: Some(prompt_version.to_string()),
        fingerprint: Some(fingerprint.to_string()),
        reasoning: suggestion.reasoning.clone(),
    }
}

/// §2.5 step 5's three outcomes: a known slug resolves; an unknown
/// (proposed-path) one needs a `new_category` review; ambiguous or
/// below-threshold needs an `ambiguous` review.
fn classify_llm_answer(
    has_known_slug: bool,
    has_proposed_path: bool,
    ambiguous: bool,
    confidence: f32,
    min_confidence: f32,
) -> (LabelStatus, Option<ReviewReason>) {
    if has_proposed_path && !has_known_slug {
        return (LabelStatus::NeedsReview, Some(ReviewReason::NewCategory));
    }
    if ambiguous || !has_known_slug || confidence < min_confidence {
        return (LabelStatus::NeedsReview, Some(ReviewReason::Ambiguous));
    }
    (LabelStatus::Resolved, None)
}

/// Turns a decided-worth-publishing [`ResolvedLabel`] into the wire
/// [`LabelRecord`] — the one place a real `category_slug` is looked up
/// (user/rule outcomes only store `category_id`; cache/LLM outcomes already
/// carry a slug-derived id but the record itself still needs the string).
pub async fn build_label_record(
    db: &impl ConnectionTrait,
    txn: &TransactionForLabeling,
    resolved: &ResolvedLabel,
) -> Result<LabelRecord, DbErr> {
    let category_slug = match resolved.category_id {
        Some(id) => proj::find_category_slug(db, id).await?,
        None => None,
    };
    Ok(LabelRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        source: txn.source.clone(),
        external_id: txn.external_id.clone(),
        category_slug,
        label_source: resolved.source,
        rule_id: resolved.rule_id,
        confidence: resolved.confidence,
        status: resolved.status,
        review_reason: resolved.review_reason,
        proposed_category_path: resolved.proposed_category_path.clone(),
        provider: resolved.provider.clone(),
        model: resolved.model.clone(),
        prompt_version: resolved.prompt_version.clone(),
        fingerprint: resolved.fingerprint.clone(),
        reasoning: resolved.reasoning.clone(),
        labeled_at: Utc::now(),
    })
}

// ---------------------------------------------------------------------------
// §2.8 rule learning trigger
// ---------------------------------------------------------------------------

/// Gathers every (`counterparty_key`, category, confidence) observation for
/// one key, from labels whose source is `user`, `llm` or `llm-cache` (never
/// `rule` — §2.8 "a rule's own output must not justify itself"), and hands
/// them to `ops.consider`. Publishes (dual-write: Kafka then the projection)
/// the candidate only when nothing exists yet for its deterministic id, or
/// the stored rule is an untouched, in-review, learned one (§2.8 "the
/// learner never overwrites a human").
pub async fn maybe_learn_rule(
    db: &impl ConnectionTrait,
    publisher: &EventPublisher,
    ops: &LabelingOps,
    counterparty_key: &str,
) -> Result<Option<RuleRecord>, DbErr> {
    let observations = proj::observations_for_counterparty_key(db, counterparty_key).await?;
    let observations: Vec<Observation<'_>> = observations
        .iter()
        .map(|(slug, confidence)| Observation {
            counterparty_key,
            category_slug: slug.as_str(),
            confidence: *confidence,
        })
        .collect();

    let Some(candidate) = (ops.consider)(&observations) else {
        return Ok(None);
    };

    let rule_id = learned_rule_uuid(&candidate.counterparty_key, &candidate.category_slug);
    if let Some(existing) = proj::find_rule(db, rule_id).await?
        && (existing.user_touched || existing.state != RuleState::InReview || existing.origin != RuleOrigin::Learned)
    {
        return Ok(None);
    }

    let now = Utc::now();
    let record = RuleRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        id: rule_id,
        name: format!("Learned: {} -> {}", candidate.counterparty_key, candidate.category_slug),
        category_slug: candidate.category_slug.clone(),
        conditions: crate::kafka::labeling::RuleConditions {
            counterparty_key: Some(candidate.counterparty_key.clone()),
            ..Default::default()
        },
        priority: 0,
        state: if candidate.auto_approved { RuleState::Active } else { RuleState::InReview },
        origin: RuleOrigin::Learned,
        auto_approved: candidate.auto_approved,
        user_touched: false,
        confidence: Some(candidate.confidence),
        evidence: None,
        created_at: now,
        revision: now,
    };

    publish_rule(publisher, &record).await;
    proj::project_rule(db, rule_id, Some(record.clone())).await?;
    Ok(Some(record))
}

async fn publish_rule(publisher: &EventPublisher, record: &RuleRecord) {
    let value = match serde_json::to_vec(record) {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, "labeler: failed to serialize learned rule, not publishing");
            return;
        }
    };
    if let Err(e) = publisher
        .publish_with_headers(
            TOPIC_RULE,
            &record.id.to_string(),
            &value,
            labeling_headers(crate::kafka::labeling::ORIGIN_LABELER),
        )
        .await
    {
        error!(error = %e, rule_id = %record.id, "labeler: failed to publish learned rule");
    }
}

/// Minimal headers for the labeling topics (§2.2): unlike the ingest topics,
/// these carry our own JSON (versioned by the payload's own
/// `schema_version`), so only `origin` matters on the wire.
fn labeling_headers(origin: &str) -> rdkafka::message::OwnedHeaders {
    use rdkafka::message::Header;
    rdkafka::message::OwnedHeaders::new().insert(Header {
        key: crate::kafka::envelope::HEADER_ORIGIN,
        value: Some(origin),
    })
}

// ---------------------------------------------------------------------------
// §2.3 publish a resolved label: dual-write (Kafka, then projection)
// ---------------------------------------------------------------------------

async fn publish_and_project_label(
    db: &impl ConnectionTrait,
    publisher: &EventPublisher,
    txn: &TransactionForLabeling,
    record: &LabelRecord,
) -> Result<(), DbErr> {
    let key = format!("{}:{}", record.source, record.external_id);
    let value = match serde_json::to_vec(record) {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, transaction_id = %txn.id, "labeler: failed to serialize label record, not publishing");
            return Ok(());
        }
    };
    if let Err(e) = publisher
        .publish_with_headers(
            TOPIC_TRANSACTION_LABEL,
            &key,
            &value,
            labeling_headers(crate::kafka::labeling::ORIGIN_LABELER),
        )
        .await
    {
        error!(error = %e, transaction_id = %txn.id, "labeler: failed to publish transaction-label");
    }
    proj::project_transaction_label(db, txn.id, Some(record.clone())).await
}

/// Resolves one transaction end to end: run the §2.5 chain, apply
/// compare-before-publish against the stored label, and — only on a genuine
/// change — dual-write the new label (and set `transaction.counterparty_key`
/// the first time it is computed). Also runs the §2.8 learner trigger when
/// this resolution came from `user`/`llm`/`llm-cache` (never `rule`).
#[allow(clippy::too_many_arguments)]
pub async fn label_one_transaction(
    db: &impl ConnectionTrait,
    publisher: &EventPublisher,
    ops: &LabelingOps,
    provider: &dyn LabelProvider,
    catalog: &CategoryCatalog,
    prompt_version: &str,
    llm_min_confidence: f32,
    cost_guard: &CostGuard,
    txn: &TransactionForLabeling,
) -> Result<(), ResolveError> {
    // Normalization is independent of resolution outcome and only needs to
    // run once per transaction — the projector's own mapper never fills it
    // in (§2.5/§3). `txn` is an immutable borrow of the sweep/consume-loop's
    // read, so a first-time computation here never lands on `txn` itself —
    // `computed_key` carries it forward for the learn-trigger below instead.
    let computed_key = if txn.counterparty_key.is_none() {
        let normalized_counterparty = (ops.normalize)(txn.counterparty_name.as_deref(), None);
        let normalized_description = (ops.normalize)(None, txn.description.as_deref());
        let key = if !normalized_counterparty.is_empty() {
            normalized_counterparty
        } else {
            normalized_description
        };
        proj::set_counterparty_key(db, txn.id, &key).await?;
        Some(key)
    } else {
        None
    };

    let Some(resolved) =
        resolve_transaction(db, ops, provider, catalog, prompt_version, llm_min_confidence, cost_guard, txn)
            .await?
    else {
        return Ok(());
    };

    let stored = proj::find_transaction_label(db, txn.id).await?;
    let stored_view = stored.as_ref().map(StoredLabel::from);
    if !should_publish(stored_view.as_ref(), &resolved) {
        return Ok(());
    }

    let record = build_label_record(db, txn, &resolved).await?;
    publish_and_project_label(db, publisher, txn, &record).await?;

    if matches!(
        resolved.source,
        LabelSource::User | LabelSource::Llm | LabelSource::LlmCache
    ) && let Some(key) = txn.counterparty_key.clone().or(computed_key)
    {
        maybe_learn_rule(db, publisher, ops, &key).await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// The sweep (§2.3)
// ---------------------------------------------------------------------------

/// Resolves every candidate the sweep query returns (unlabelled, or held
/// with `review_reason = provider_error`), bounded by `limit` — the only
/// mechanism that makes a failed label eventually succeed (§2.3).
#[allow(clippy::too_many_arguments)]
pub async fn run_sweep(
    db: &impl ConnectionTrait,
    publisher: &EventPublisher,
    ops: &LabelingOps,
    provider: &dyn LabelProvider,
    catalog: &CategoryCatalog,
    prompt_version: &str,
    llm_min_confidence: f32,
    cost_guard: &CostGuard,
    limit: u64,
) -> Result<usize, ResolveError> {
    let candidates = proj::sweep_candidates(db, limit).await?;
    let mut labeled = 0usize;
    for transaction_id in candidates {
        if cost_guard.is_exhausted() {
            info!("labeler: sweep stopping, cost guard exhausted");
            break;
        }
        let Some(txn) = proj::find_transaction(db, transaction_id).await? else {
            continue;
        };
        label_one_transaction(
            db,
            publisher,
            ops,
            provider,
            catalog,
            prompt_version,
            llm_min_confidence,
            cost_guard,
            &txn,
        )
        .await?;
        labeled += 1;
    }
    Ok(labeled)
}

// ---------------------------------------------------------------------------
// §2.3 projector-lag startup guard
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ProjectionLag {
    pub topic: String,
    pub committed: i64,
    pub high_watermark: i64,
}

/// Checks `projection_offset` (the three ingest topics under the main
/// projector's own keys, plus `transaction-label`/`llm-cache` under this
/// labeler's own `@labeler`-suffixed bookkeeping) against each topic's
/// broker high watermark. `Ok(())` only when every topic is within
/// `max_lag` records of caught up; otherwise the labeler must refuse to
/// start (§2.3) — calling this is the caller's job, not this function's.
pub async fn check_projection_lag(
    db: &DatabaseConnection,
    brokers: &str,
    max_lag: u64,
) -> Result<(), Vec<ProjectionLag>> {
    use crate::kafka::envelope::{TOPIC_ACCOUNT, TOPIC_ACCOUNT_BALANCE};

    let stored = offsets::load_offsets(db)
        .await
        .map_err(|e| vec![ProjectionLag { topic: format!("<offsets query failed: {e}>"), committed: 0, high_watermark: 0 }])?;

    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers)
        .set("group.id", "finreport-labeler-lag-check")
        .create()
        .map_err(|e| vec![ProjectionLag { topic: format!("<kafka client error: {e}>"), committed: 0, high_watermark: 0 }])?;

    // The three ingest topics, checked under the *main projector's* own
    // (unsuffixed) offset keys: the labeler must not run ahead of it.
    let ingest_checks: Vec<(String, String)> = [TOPIC_ACCOUNT, TOPIC_ACCOUNT_BALANCE, TOPIC_TRANSACTION]
        .into_iter()
        .map(|t| (t.to_string(), t.to_string()))
        .collect();
    // `transaction-label`/`llm-cache`, checked under the *main projector's*
    // own (unsuffixed) offset keys, now that the projector also consumes
    // and projects these two topics alongside the three ingest ones
    // (`projection::LABELING_PROJECTION_TOPICS`) — the labeler must not run
    // ahead of the projector's own projection of its own published labels,
    // the same reasoning as `ingest_checks` above. Nothing ever advances an
    // `@labeler`-suffixed offset for these two topics: the labeler only
    // *publishes* to them, it never consumes them back.
    let own_checks: Vec<(String, String)> = [TOPIC_TRANSACTION_LABEL, TOPIC_LLM_CACHE]
        .into_iter()
        .map(|t| (t.to_string(), t.to_string()))
        .collect();

    let mut lagging = Vec::new();
    for (topic, offset_key) in ingest_checks.into_iter().chain(own_checks) {
        let (_, high) = match consumer.fetch_watermarks(&topic, LABELER_PARTITION, Duration::from_secs(10)) {
            Ok(w) => w,
            Err(e) => {
                lagging.push(ProjectionLag { topic: topic.clone(), committed: -1, high_watermark: -1 });
                warn!(%topic, error = %e, "labeler: could not fetch watermark for lag check");
                continue;
            }
        };
        let committed = stored.get(&(offset_key, LABELER_PARTITION)).copied().unwrap_or(0);
        let lag = (high - committed).max(0) as u64;
        if lag > max_lag {
            lagging.push(ProjectionLag { topic, committed, high_watermark: high });
        }
    }

    if lagging.is_empty() {
        Ok(())
    } else {
        Err(lagging)
    }
}

// ---------------------------------------------------------------------------
// The Kafka-facing consume loop
// ---------------------------------------------------------------------------

pub struct LabelerConfig {
    pub brokers: String,
    pub batch_max_records: usize,
    pub batch_max_wait: Duration,
    pub llm_max_requests_per_run: u32,
    pub prompt_version: String,
    pub llm_min_confidence: f32,
    pub until_caught_up: bool,
}

#[derive(Debug)]
pub enum LabelerError {
    Kafka(rdkafka::error::KafkaError),
    Db(DbErr),
}

impl From<DbErr> for LabelerError {
    fn from(e: DbErr) -> Self {
        LabelerError::Db(e)
    }
}

impl std::fmt::Display for LabelerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LabelerError::Kafka(e) => write!(f, "kafka error: {e}"),
            LabelerError::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for LabelerError {}

impl From<ResolveError> for LabelerError {
    fn from(e: ResolveError) -> Self {
        match e {
            ResolveError::Db(e) => LabelerError::Db(e),
        }
    }
}

struct ConsumedLabelingRecord {
    topic: String,
    offset: i64,
    key: Option<String>,
    payload: Vec<u8>,
}

impl ConsumedLabelingRecord {
    fn from_message(message: &BorrowedMessage<'_>) -> Self {
        Self {
            topic: message.topic().to_string(),
            offset: message.offset(),
            key: message.key().map(|k| String::from_utf8_lossy(k).into_owned()),
            payload: message.payload().map(|p| p.to_vec()).unwrap_or_default(),
        }
    }
}

/// Runs the labeler loop: consumes [`LABELER_INPUT_TOPICS`], resolves and
/// (compare-before-publish) publishes labels, runs the sweep at startup and
/// after every caught-up pass, until caught up (`config.until_caught_up`) or
/// forever.
pub async fn run(
    db: DatabaseConnection,
    publisher: EventPublisher,
    provider: Box<dyn LabelProvider>,
    ops: LabelingOps,
    config: LabelerConfig,
) -> Result<(), LabelerError> {
    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &config.brokers)
        .set("group.id", "finreport-labeler")
        .set("enable.auto.commit", "false")
        .set("enable.partition.eof", "false")
        .create()
        .map_err(LabelerError::Kafka)?;

    let stored_offsets = offsets::load_offsets(&db).await?;
    let mut tpl = TopicPartitionList::new();
    for topic in LABELER_INPUT_TOPICS {
        let offset = stored_offsets
            .get(&(offset_topic_key(topic), LABELER_PARTITION))
            .map(|&next| Offset::Offset(next))
            .unwrap_or(Offset::Beginning);
        tpl.add_partition_offset(topic, LABELER_PARTITION, offset)
            .map_err(LabelerError::Kafka)?;
    }
    consumer.assign(&tpl).map_err(LabelerError::Kafka)?;

    let catalog = proj::build_catalog(&db).await?;
    let cost_guard = CostGuard::new(config.llm_max_requests_per_run);

    // Sweep once at startup, per §2.3 ("at the start of every run").
    run_sweep(
        &db,
        &publisher,
        &ops,
        provider.as_ref(),
        &catalog,
        &config.prompt_version,
        config.llm_min_confidence,
        &cost_guard,
        u64::from(config.llm_max_requests_per_run),
    )
    .await?;
    // Detection (iteration 3 §3): a post-batch pass "invoked after the rule
    // learner" — same cadence as the sweep above.
    crate::detect::processor::run_detection_pass(&db, &publisher).await?;

    let mut next_offsets: HashMap<String, i64> = HashMap::new();
    let high_watermarks = if config.until_caught_up {
        let mut marks = HashMap::new();
        for topic in LABELER_INPUT_TOPICS {
            let (low, high) = consumer
                .fetch_watermarks(topic, LABELER_PARTITION, Duration::from_secs(10))
                .map_err(LabelerError::Kafka)?;
            marks.insert(topic.to_string(), high);
            let starting = stored_offsets
                .get(&(offset_topic_key(topic), LABELER_PARTITION))
                .copied()
                .unwrap_or(low);
            next_offsets.insert(topic.to_string(), starting);
        }
        Some(marks)
    } else {
        None
    };

    loop {
        let batch = collect_batch(&consumer, config.batch_max_records, config.batch_max_wait).await;

        if batch.is_empty() {
            if let Some(marks) = &high_watermarks
                && is_caught_up(&next_offsets, marks)
            {
                run_sweep(
                    &db,
                    &publisher,
                    &ops,
                    provider.as_ref(),
                    &catalog,
                    &config.prompt_version,
                    config.llm_min_confidence,
                    &cost_guard,
                    u64::from(config.llm_max_requests_per_run),
                )
                .await?;
                crate::detect::processor::run_detection_pass(&db, &publisher).await?;
                info!("labeler: caught up with all input topics, exiting (--until-caught-up)");
                return Ok(());
            }
            continue;
        }

        for record in &batch {
            next_offsets.insert(record.topic.clone(), record.offset + 1);
            process_record(&db, &publisher, &ops, provider.as_ref(), &catalog, &config, &cost_guard, record).await?;
        }

        let txn = db.begin().await.map_err(LabelerError::Db)?;
        for record in &batch {
            offsets::commit_offset(
                &txn,
                &offset_topic_key(&record.topic),
                LABELER_PARTITION,
                record.offset + 1,
                Utc::now(),
            )
            .await
            .map_err(LabelerError::Db)?;
        }
        txn.commit().await.map_err(LabelerError::Db)?;
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_record(
    db: &DatabaseConnection,
    publisher: &EventPublisher,
    ops: &LabelingOps,
    provider: &dyn LabelProvider,
    catalog: &CategoryCatalog,
    config: &LabelerConfig,
    cost_guard: &CostGuard,
    record: &ConsumedLabelingRecord,
) -> Result<(), LabelerError> {
    match record.topic.as_str() {
        t if t == TOPIC_TRANSACTION => {
            handle_transaction_record(db, publisher, ops, provider, catalog, config, cost_guard, record).await
        }
        t if t == TOPIC_USER_LABEL => handle_user_label_record(db, publisher, ops, provider, catalog, config, cost_guard, record).await,
        t if t == TOPIC_RULE => handle_rule_record(db, publisher, ops, provider, catalog, config, cost_guard, record).await,
        t if t == TOPIC_LABEL_REQUEST => {
            handle_label_request_record(db, publisher, ops, provider, catalog, config, cost_guard, record).await
        }
        other => {
            warn!(topic = other, "labeler: record on an unrecognized topic, skipped");
            Ok(())
        }
    }
}

fn parse_source_external_id(key: &str) -> Option<(&str, &str)> {
    key.split_once(':')
}

#[allow(clippy::too_many_arguments)]
async fn resolve_and_publish_by_source_external_id(
    db: &DatabaseConnection,
    publisher: &EventPublisher,
    ops: &LabelingOps,
    provider: &dyn LabelProvider,
    catalog: &CategoryCatalog,
    config: &LabelerConfig,
    cost_guard: &CostGuard,
    source: &str,
    external_id: &str,
) -> Result<(), LabelerError> {
    let Some(txn) = proj::find_transaction_by_source(db, source, external_id).await? else {
        warn!(source, external_id, "labeler: no projected transaction for this key yet, skipping");
        return Ok(());
    };
    label_one_transaction(
        db,
        publisher,
        ops,
        provider,
        catalog,
        &config.prompt_version,
        config.llm_min_confidence,
        cost_guard,
        &txn,
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_transaction_record(
    db: &DatabaseConnection,
    publisher: &EventPublisher,
    ops: &LabelingOps,
    provider: &dyn LabelProvider,
    catalog: &CategoryCatalog,
    config: &LabelerConfig,
    cost_guard: &CostGuard,
    record: &ConsumedLabelingRecord,
) -> Result<(), LabelerError> {
    // The main projector's `MapperRegistry` has already normalized this
    // transaction by the time the labeler sees it (the labeler contains no
    // per-source code, §2.3) — only its already-projected identity is
    // needed here, so the key (`<source>:<external_id>`) is enough to look
    // it back up rather than re-mapping the payload ourselves.
    let Some(key) = record.key.as_deref() else {
        warn!("labeler: transaction record with no key, skipped");
        return Ok(());
    };
    let Some((source, external_id)) = parse_source_external_id(key) else {
        warn!(key, "labeler: transaction record key not in <source>:<external_id> form, skipped");
        return Ok(());
    };
    resolve_and_publish_by_source_external_id(
        db, publisher, ops, provider, catalog, config, cost_guard, source, external_id,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn handle_user_label_record(
    db: &DatabaseConnection,
    publisher: &EventPublisher,
    ops: &LabelingOps,
    provider: &dyn LabelProvider,
    catalog: &CategoryCatalog,
    config: &LabelerConfig,
    cost_guard: &CostGuard,
    record: &ConsumedLabelingRecord,
) -> Result<(), LabelerError> {
    let envelope_origin = crate::kafka::labeling::ORIGIN_USER;
    let _ = envelope_origin; // documented in the module header, not re-checked here: WP4 is the only producer.

    if record.payload.is_empty() {
        // A tombstone: the transaction is gone. The transaction topic's own
        // tombstone (consumed by the main projector) handles deleting the
        // row; nothing for the labeler to resolve.
        proj::project_user_label(db, tombstone_id_from_key(record)?, None).await?;
        return Ok(());
    }

    let parsed: UserLabelRecord = match serde_json::from_slice(&record.payload) {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, "labeler: poison user-label record, skipped");
            return Ok(());
        }
    };

    let transaction_id = crate::kafka::envelope::transaction_uuid(&parsed.source, &parsed.external_id);
    proj::project_user_label(db, transaction_id, Some(parsed.clone())).await?;

    resolve_and_publish_by_source_external_id(
        db, publisher, ops, provider, catalog, config, cost_guard, &parsed.source, &parsed.external_id,
    )
    .await
}

fn tombstone_id_from_key(record: &ConsumedLabelingRecord) -> Result<Uuid, LabelerError> {
    let key = record.key.as_deref().unwrap_or_default();
    let Some((source, external_id)) = parse_source_external_id(key) else {
        return Err(LabelerError::Db(DbErr::Custom(format!(
            "user-label tombstone key {key:?} not in <source>:<external_id> form"
        ))));
    };
    Ok(crate::kafka::envelope::transaction_uuid(source, external_id))
}

#[allow(clippy::too_many_arguments)]
async fn handle_rule_record(
    db: &DatabaseConnection,
    publisher: &EventPublisher,
    ops: &LabelingOps,
    provider: &dyn LabelProvider,
    catalog: &CategoryCatalog,
    config: &LabelerConfig,
    cost_guard: &CostGuard,
    record: &ConsumedLabelingRecord,
) -> Result<(), LabelerError> {
    let Some(key) = record.key.as_deref() else {
        warn!("labeler: rule record with no key, skipped");
        return Ok(());
    };
    let Ok(rule_id) = Uuid::parse_str(key) else {
        warn!(key, "labeler: rule record key is not a UUID, skipped");
        return Ok(());
    };

    if record.payload.is_empty() {
        proj::project_rule(db, rule_id, None).await?;
        return reresolve_rule_affected_set(db, publisher, ops, provider, catalog, config, cost_guard, rule_id).await;
    }

    let parsed: RuleRecord = match serde_json::from_slice(&record.payload) {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, "labeler: poison rule record, skipped");
            return Ok(());
        }
    };
    proj::project_rule(db, rule_id, Some(parsed)).await?;
    reresolve_rule_affected_set(db, publisher, ops, provider, catalog, config, cost_guard, rule_id).await
}

/// §2.3 "a rule record resolves every transaction the rule matches now **or**
/// matched before the edit" — re-resolves every transaction currently
/// labelled by this rule. Transactions the *edited* rule newly matches but
/// never matched before are picked up by the ordinary `transaction`-topic
/// path (nothing re-scans the whole table here) or the sweep.
#[allow(clippy::too_many_arguments)]
async fn reresolve_rule_affected_set(
    db: &DatabaseConnection,
    publisher: &EventPublisher,
    ops: &LabelingOps,
    provider: &dyn LabelProvider,
    catalog: &CategoryCatalog,
    config: &LabelerConfig,
    cost_guard: &CostGuard,
    rule_id: Uuid,
) -> Result<(), LabelerError> {
    let affected = proj::transaction_ids_labelled_by_rule(db, rule_id).await?;
    for transaction_id in affected {
        let Some(txn) = proj::find_transaction(db, transaction_id).await? else {
            continue;
        };
        label_one_transaction(
            db,
            publisher,
            ops,
            provider,
            catalog,
            &config.prompt_version,
            config.llm_min_confidence,
            cost_guard,
            &txn,
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_label_request_record(
    db: &DatabaseConnection,
    publisher: &EventPublisher,
    ops: &LabelingOps,
    provider: &dyn LabelProvider,
    catalog: &CategoryCatalog,
    config: &LabelerConfig,
    cost_guard: &CostGuard,
    record: &ConsumedLabelingRecord,
) -> Result<(), LabelerError> {
    if record.payload.is_empty() {
        // `label-request` is a work queue (§2.2): nothing projects a
        // tombstone for it.
        return Ok(());
    }
    let parsed: LabelRequestRecord = match serde_json::from_slice(&record.payload) {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, "labeler: poison label-request record, skipped");
            return Ok(());
        }
    };
    match parsed.target {
        LabelRequestTarget::Transaction { source, external_id } => {
            resolve_and_publish_by_source_external_id(
                db, publisher, ops, provider, catalog, config, cost_guard, &source, &external_id,
            )
            .await
        }
        LabelRequestTarget::Rule { rule_id } => {
            reresolve_rule_affected_set(db, publisher, ops, provider, catalog, config, cost_guard, rule_id).await
        }
    }
}

async fn collect_batch(
    consumer: &StreamConsumer,
    max_records: usize,
    max_wait: Duration,
) -> Vec<ConsumedLabelingRecord> {
    let mut batch = Vec::new();
    let deadline = tokio::time::Instant::now() + max_wait;

    loop {
        if batch.len() >= max_records {
            break;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        match tokio::time::timeout(remaining, consumer.recv()).await {
            Ok(Ok(message)) => batch.push(ConsumedLabelingRecord::from_message(&message)),
            Ok(Err(e)) => warn!(error = %e, "labeler: kafka poll error"),
            Err(_elapsed) => break,
        }
    }

    batch
}

fn is_caught_up(next_offsets: &HashMap<String, i64>, high_watermarks: &HashMap<String, i64>) -> bool {
    for topic in LABELER_INPUT_TOPICS {
        let high = *high_watermarks.get(topic).unwrap_or(&0);
        if high == 0 {
            continue;
        }
        let current = next_offsets.get(topic).copied().unwrap_or(0);
        if current < high {
            return false;
        }
    }
    true
}

// Suppresses an `unused_imports` warning for the envelope parsing re-export
// used only by `#[cfg(test)]` helpers in sibling integration tests, not here.
#[allow(unused_imports)]
use Envelope as _EnvelopeReexportForTests;

#[cfg(test)]
mod tests {
    //! Pure-logic unit tests: compare-before-publish and the cost guard.
    //! Neither needs WP2's real (panicking) stubs — `should_publish` takes
    //! plain data, and `CostGuard` has no WP2 dependency at all.

    use super::*;

    fn resolved(source: LabelSource, category_id: Option<Uuid>) -> ResolvedLabel {
        ResolvedLabel {
            status: LabelStatus::Resolved,
            source,
            category_id,
            rule_id: None,
            confidence: None,
            review_reason: None,
            proposed_category_path: None,
            provider: None,
            model: None,
            prompt_version: None,
            fingerprint: None,
            reasoning: None,
        }
    }

    #[test]
    fn no_stored_label_always_publishes() {
        let new = resolved(LabelSource::Rule, Some(Uuid::new_v4()));
        assert!(should_publish(None, &new));
    }

    #[test]
    fn llm_recomputing_as_llm_cache_for_the_same_fingerprint_is_not_a_change() {
        let category_id = Uuid::new_v4();
        let stored = StoredLabel {
            source: LabelSource::Llm,
            category_id: Some(category_id),
            rule_id: None,
            status: LabelStatus::Resolved,
            review_reason: None,
            proposed_category_path: None,
            fingerprint: Some("fp-1".to_string()),
        };
        let new = ResolvedLabel {
            fingerprint: Some("fp-1".to_string()),
            ..resolved(LabelSource::LlmCache, Some(category_id))
        };

        assert!(
            !should_publish(Some(&stored), &new),
            "an llm-cache hit reproducing an identical llm answer must not republish"
        );
    }

    #[test]
    fn a_changed_fingerprint_does_republish() {
        let category_id = Uuid::new_v4();
        let stored = StoredLabel {
            source: LabelSource::Llm,
            category_id: Some(category_id),
            rule_id: None,
            status: LabelStatus::Resolved,
            review_reason: None,
            proposed_category_path: None,
            fingerprint: Some("fp-1".to_string()),
        };
        let new = ResolvedLabel {
            fingerprint: Some("fp-2".to_string()),
            ..resolved(LabelSource::Llm, Some(category_id))
        };

        assert!(should_publish(Some(&stored), &new), "a changed fingerprint (new provider/model/prompt) must republish");
    }

    #[test]
    fn a_rule_that_now_wins_over_a_stored_llm_label_republishes() {
        let stored = StoredLabel {
            source: LabelSource::Llm,
            category_id: Some(Uuid::new_v4()),
            rule_id: None,
            status: LabelStatus::Resolved,
            review_reason: None,
            proposed_category_path: None,
            fingerprint: Some("fp-1".to_string()),
        };
        let new_category = Uuid::new_v4();
        let mut new = resolved(LabelSource::Rule, Some(new_category));
        new.rule_id = Some(Uuid::new_v4());

        assert!(should_publish(Some(&stored), &new));
    }

    #[test]
    fn an_unchanged_rule_label_does_not_republish() {
        let rule_id = Uuid::new_v4();
        let category_id = Uuid::new_v4();
        let stored = StoredLabel {
            source: LabelSource::Rule,
            category_id: Some(category_id),
            rule_id: Some(rule_id),
            status: LabelStatus::Resolved,
            review_reason: None,
            proposed_category_path: None,
            fingerprint: None,
        };
        let mut new = resolved(LabelSource::Rule, Some(category_id));
        new.rule_id = Some(rule_id);

        assert!(!should_publish(Some(&stored), &new));
    }

    #[test]
    fn cost_guard_stops_at_the_configured_limit() {
        let guard = CostGuard::new(2);
        assert!(guard.try_consume());
        assert!(guard.try_consume());
        assert!(!guard.try_consume(), "the third call must be refused once the budget is spent");
        assert!(guard.is_exhausted());
    }

    #[test]
    fn classify_llm_answer_covers_every_review_reason() {
        assert_eq!(
            classify_llm_answer(false, true, false, 0.8, 0.5),
            (LabelStatus::NeedsReview, Some(ReviewReason::NewCategory))
        );
        assert_eq!(
            classify_llm_answer(false, false, true, 0.8, 0.5),
            (LabelStatus::NeedsReview, Some(ReviewReason::Ambiguous))
        );
        assert_eq!(
            classify_llm_answer(true, false, false, 0.2, 0.5),
            (LabelStatus::NeedsReview, Some(ReviewReason::Ambiguous)),
            "below APP_llm_min_confidence must be held for review even with a known slug"
        );
        assert_eq!(classify_llm_answer(true, false, false, 0.9, 0.5), (LabelStatus::Resolved, None));
    }

    /// Exercises `resolve_transaction`'s orchestration (which precedence
    /// step wins) with fake `LabelingOps`, independent of WP2's real
    /// (panicking) `normalize`/`fingerprint`/`most_specific_match`.
    #[tokio::test]
    async fn resolve_transaction_short_circuits_on_a_cache_hit_without_calling_the_provider() {
        use entity::entities::llm_label_cache;
        use sea_orm::{DatabaseBackend, MockDatabase};

        let category_id = Uuid::new_v4();
        let cache_row = llm_label_cache::Model {
            fingerprint: "deterministic-fp".to_string(),
            category_id: Some(category_id),
            proposed_path: None,
            confidence: rust_decimal::Decimal::new(900, 3),
            provider: "fake".to_string(),
            model: "fake-v1".to_string(),
            prompt_version: "1".to_string(),
            reasoning: None,
            created_at: Utc::now().into(),
        };

        // Queries in order: find_valid_splits, find_user_label, active_rules,
        // find_cache. The fake ops never call the provider (asserted via the
        // panicking provider below), so only the cache row needs mocking.
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([Vec::<entity::entities::transaction_split::Model>::new()])
            .append_query_results([Vec::<entity::entities::transaction_user_label::Model>::new()])
            .append_query_results([Vec::<entity::entities::rule::Model>::new()])
            .append_query_results([vec![cache_row]])
            .into_connection();

        let ops = LabelingOps::fake(
            |_, _| "deterministic-key".to_string(),
            |_, _, _, _, _, _| "deterministic-fp".to_string(),
            |_, _| None,
            |_| None,
        );

        struct PanicProvider;
        #[async_trait::async_trait]
        impl LabelProvider for PanicProvider {
            fn id(&self) -> &'static str {
                "panic"
            }
            fn model(&self) -> &str {
                "panic"
            }
            async fn suggest(
                &self,
                _req: &LabelRequest<'_>,
            ) -> Result<categorizer::provider::LabelSuggestion, categorizer::provider::ProviderError> {
                panic!("a cache hit must short-circuit before ever calling the provider");
            }
        }

        let txn = TransactionForLabeling {
            id: Uuid::new_v4(),
            source: "comdirect".to_string(),
            external_id: "ACC1-TEST".to_string(),
            account_id: Uuid::new_v4(),
            amount: rust_decimal::Decimal::new(-1000, 2),
            currency: "EUR".to_string(),
            counterparty_name: Some("Some Merchant".to_string()),
            counterparty_iban: None,
            description: None,
            transaction_type: None,
            counterparty_key: None,
            booking_date: chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        };
        let catalog = CategoryCatalog::default();
        let cost_guard = CostGuard::new(10);

        let result = resolve_transaction(&db, &ops, &PanicProvider, &catalog, "1", 0.5, &cost_guard, &txn)
            .await
            .expect("resolution must succeed");

        let resolved = result.expect("a cache hit resolves to Some");
        assert_eq!(resolved.source, LabelSource::LlmCache);
        assert_eq!(resolved.category_id, Some(category_id));
    }
}
