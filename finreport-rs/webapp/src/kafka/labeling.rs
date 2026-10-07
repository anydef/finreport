//! Iteration 2 §2.2/§2.4/§2.6/§2.7: the labeling topic constants and the event
//! structs/serde published on them.
//!
//! These topics carry **our own JSON**, not a bank's, so unlike the ingest
//! topics (§2.2 `envelope.rs`) they are versioned by a `schema_version` field
//! in the payload itself, in addition to the iteration-1 envelope headers
//! (`origin` is `labeler` for label/cache records, `user` for the
//! human-edited ones). A tombstone (null value) means "this entity no longer
//! exists" and projects to a delete.
//!
//! Frozen by WP0. Consumed by WP2 (resolution/rules/learner, producer side),
//! WP3 (labeler processor + projections, both sides) and WP4 (GraphQL
//! mutations, producer side for `user-label`/`rule`/`label-request`).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::kafka::envelope::FINREPORT_NS;

// ---------------------------------------------------------------------------
// Topics (§2.2)
// ---------------------------------------------------------------------------

/// Labeler output: one row per transaction's current label. Keyed
/// `<source>:<external_id>`, compacted.
pub const TOPIC_TRANSACTION_LABEL: &str = "finreport.transaction-label";
/// One LLM answer per distinct `fingerprint` (§2.4). Keyed by `fingerprint`,
/// compacted. Deliberately separate from `transaction-label` — see §2.2.
pub const TOPIC_LLM_CACHE: &str = "finreport.llm-cache";
/// Human overrides and splits (§2.6). Keyed `<source>:<external_id>`,
/// compacted.
pub const TOPIC_USER_LABEL: &str = "finreport.user-label";
/// Rule state (§2.7). Keyed by the rule's UUID, compacted.
pub const TOPIC_RULE: &str = "finreport.rule";
/// Category tree nodes (§3). Keyed by the category's UUID, compacted.
pub const TOPIC_CATEGORY: &str = "finreport.category";
/// Explicit re-resolution requests (§2.3): a work queue, not state. Keyed
/// `<source>:<external_id>` or `reapply:<rule-id>`; time-retained (7 d) and
/// replaceable, unlike the other five topics.
pub const TOPIC_LABEL_REQUEST: &str = "finreport.label-request";

/// `origin` header value for label and cache records (§2.2).
pub const ORIGIN_LABELER: &str = "labeler";
/// `origin` header value for the human-edited topics (`user-label`, `rule`,
/// `category` when user-created) (§2.2).
pub const ORIGIN_USER: &str = "user";

/// Current payload schema version for every struct in this module. Bumped
/// only when a payload shape changes, independent of the envelope header's
/// own `schema_version` (§2.2).
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Deterministic id for a learned rule (§2.8, §3):
/// `UUIDv5(FINREPORT_NS, "rule\0"+counterparty_key+"\0"+category_slug)` — so
/// the learner cannot create two rules for the same evidence across a
/// restart or a replay.
pub fn learned_rule_uuid(counterparty_key: &str, category_slug: &str) -> Uuid {
    let name = format!("rule\0{counterparty_key}\0{category_slug}");
    Uuid::new_v5(&FINREPORT_NS, name.as_bytes())
}

/// Deterministic id for a category node (§3):
/// `UUIDv5(FINREPORT_NS, "category\0"+slug)` — so `category-seed` is
/// idempotent (same slugs ⇒ same ids).
pub fn category_uuid(slug: &str) -> Uuid {
    let name = format!("category\0{slug}");
    Uuid::new_v5(&FINREPORT_NS, name.as_bytes())
}

/// Deterministic id for a `transaction_split` row (§3):
/// `UUIDv5(txn_id, part_index)`.
pub fn split_uuid(transaction_id: Uuid, part_index: i32) -> Uuid {
    let name = format!("{transaction_id}\0{part_index}");
    Uuid::new_v5(&FINREPORT_NS, name.as_bytes())
}

// ---------------------------------------------------------------------------
// §2.4 Label and cache records
// ---------------------------------------------------------------------------

/// `label_source` values (§3 `transaction_label.label_source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LabelSource {
    User,
    Rule,
    LlmCache,
    Llm,
}

/// `status` values (§3 `transaction_label.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelStatus {
    Resolved,
    NeedsReview,
}

/// `review_reason` values (§3 `transaction_label.review_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    Ambiguous,
    NewCategory,
    ProviderError,
    SplitMismatch,
}

/// Published on [`TOPIC_TRANSACTION_LABEL`] (§2.4). Mirrors the
/// `transaction_label` columns (§3) plus the transaction identity and
/// `schema_version`. Categories are referenced by **slug**, not UUID, so the
/// log stays readable and a rebuild resolves slugs through the category tree
/// it has just built.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelRecord {
    pub schema_version: u32,
    pub source: String,
    pub external_id: String,
    /// `null` unless `status == Resolved`.
    pub category_slug: Option<String>,
    pub label_source: LabelSource,
    pub rule_id: Option<Uuid>,
    /// `null` unless the label came from the LLM or its cache.
    pub confidence: Option<f32>,
    pub status: LabelStatus,
    pub review_reason: Option<ReviewReason>,
    /// An LLM suggestion that is **never** auto-created.
    pub proposed_category_path: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    pub fingerprint: Option<String>,
    pub reasoning: Option<String>,
    pub labeled_at: DateTime<Utc>,
}

/// Published on [`TOPIC_LLM_CACHE`] (§2.4), keyed by `fingerprint`. Mirrors
/// `llm_label_cache` (§3): the provider's answer and nothing about which
/// transaction provoked it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheRecord {
    pub schema_version: u32,
    pub fingerprint: String,
    pub category_slug: Option<String>,
    pub proposed_path: Option<String>,
    pub confidence: f32,
    pub provider: String,
    pub model: String,
    pub prompt_version: String,
    pub reasoning: Option<String>,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// §2.6 User label record (overrides + splits)
// ---------------------------------------------------------------------------

/// One part of a split transaction (§2.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitPart {
    pub index: i32,
    /// `NUMERIC(20,4)`, same sign as the transaction, exact — no tolerance.
    pub amount: Decimal,
    pub category_slug: String,
}

/// Published on [`TOPIC_USER_LABEL`] (§2.2, §2.6). Keyed
/// `<source>:<external_id>`. `category_slug: None` together with an empty
/// `parts` is a real event ("clear the override, fall back to the next
/// source"), distinct from a tombstone ("the transaction is gone").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserLabelRecord {
    pub schema_version: u32,
    pub source: String,
    pub external_id: String,
    /// `null` when parts are present.
    pub category_slug: Option<String>,
    #[serde(default)]
    pub parts: Vec<SplitPart>,
    /// RFC 3339; last-writer-wins (§2.1).
    pub revision: DateTime<Utc>,
    pub note: Option<String>,
}

// ---------------------------------------------------------------------------
// §2.7 Rules
// ---------------------------------------------------------------------------

/// `state` values (§3 `rule.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleState {
    Active,
    InReview,
    Revoked,
    Rejected,
}

/// `origin` values (§3 `rule.origin`) — distinct from the Kafka envelope
/// `origin` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOrigin {
    User,
    Learned,
}

/// `conditions` (§2.7): all present fields AND together. The only shape
/// defined nowhere else in the spec.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuleConditions {
    /// Normalized, exact match against `transaction.counterparty_key`.
    pub counterparty_key: Option<String>,
    pub counterparty_iban: Option<String>,
    /// Compiled with a size limit; rejected at the mutation if it does not
    /// compile (§2.7).
    pub description_regex: Option<String>,
    pub description_contains: Option<String>,
    pub direction: Option<String>,
    pub amount_min: Option<Decimal>,
    pub amount_max: Option<Decimal>,
    pub account_ids: Option<Vec<Uuid>>,
}

/// Published on [`TOPIC_RULE`] (§2.2, §2.7). Keyed by the rule's UUID. Mirrors
/// the `rule` columns (§3) by `category_slug`, not id, plus `schema_version`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleRecord {
    pub schema_version: u32,
    pub id: Uuid,
    pub name: String,
    pub category_slug: String,
    pub conditions: RuleConditions,
    pub priority: i32,
    pub state: RuleState,
    pub origin: RuleOrigin,
    pub auto_approved: bool,
    pub user_touched: bool,
    pub confidence: Option<f32>,
    pub evidence: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    /// RFC 3339; last-writer-wins (§2.1).
    pub revision: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// §3 Category
// ---------------------------------------------------------------------------

/// `kind` values (§3 `category.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CategoryKind {
    Income,
    Expense,
    Transfer,
    Saving,
}

/// `origin` values (§3 `category.origin`) — distinct from the Kafka envelope
/// `origin` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CategoryOrigin {
    Seed,
    User,
}

/// Published on [`TOPIC_CATEGORY`] (§2.2, §3). Keyed by the category's UUID
/// ([`category_uuid`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryRecord {
    pub schema_version: u32,
    pub id: Uuid,
    pub slug: String,
    pub parent_slug: Option<String>,
    pub name: String,
    pub kind: CategoryKind,
    pub depth: i16,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default)]
    pub archived: bool,
    pub origin: CategoryOrigin,
    /// Reserved per-tenant seam (§3); always `None` this iteration.
    pub owner_user_id: Option<Uuid>,
    /// RFC 3339; last-writer-wins (§2.1).
    pub revision: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// §2.3 Label request (work queue, not state)
// ---------------------------------------------------------------------------

/// What a [`LabelRequestRecord`] asks the labeler to re-resolve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LabelRequestTarget {
    /// Keyed `<source>:<external_id>`: resolve this one transaction.
    Transaction { source: String, external_id: String },
    /// Keyed `reapply:<rule-id>`: resolve every transaction the rule matches
    /// now or matched before (`transaction_label.rule_id = id`).
    Rule { rule_id: Uuid },
}

/// Published on [`TOPIC_LABEL_REQUEST`] (§2.2, §2.3). Not a projection — the
/// labeler's own work queue, never consumed by the projector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelRequestRecord {
    pub schema_version: u32,
    #[serde(flatten)]
    pub target: LabelRequestTarget,
    pub requested_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learned_rule_ids_are_deterministic_and_scoped() {
        let a = learned_rule_uuid("lidl", "food.groceries");
        let b = learned_rule_uuid("lidl", "food.groceries");
        assert_eq!(a, b);

        let other_category = learned_rule_uuid("lidl", "food.restaurants");
        assert_ne!(a, other_category);

        let other_key = learned_rule_uuid("rewe", "food.groceries");
        assert_ne!(a, other_key);
    }

    #[test]
    fn category_ids_are_deterministic() {
        let a = category_uuid("food.groceries");
        let b = category_uuid("food.groceries");
        assert_eq!(a, b);
        assert_ne!(a, category_uuid("food.restaurants"));
    }

    #[test]
    fn split_ids_are_scoped_by_transaction_and_index() {
        let txn = Uuid::new_v4();
        let s0 = split_uuid(txn, 0);
        let s1 = split_uuid(txn, 1);
        assert_ne!(s0, s1);
        assert_eq!(s0, split_uuid(txn, 0));
    }

    #[test]
    fn label_record_round_trips_through_json() {
        let record = LabelRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            source: "comdirect".to_string(),
            external_id: "ACC1-SPEND-00".to_string(),
            category_slug: Some("food.groceries".to_string()),
            label_source: LabelSource::Rule,
            rule_id: Some(Uuid::new_v4()),
            confidence: None,
            status: LabelStatus::Resolved,
            review_reason: None,
            proposed_category_path: None,
            provider: None,
            model: None,
            prompt_version: None,
            fingerprint: None,
            reasoning: None,
            labeled_at: Utc::now(),
        };

        let json = serde_json::to_string(&record).unwrap();
        let round_tripped: LabelRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, round_tripped);
    }

    #[test]
    fn cache_record_round_trips_through_json() {
        let record = CacheRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            fingerprint: "abc123".to_string(),
            category_slug: Some("food.groceries".to_string()),
            proposed_path: None,
            confidence: 0.87,
            provider: "fake".to_string(),
            model: "fake-v1".to_string(),
            prompt_version: "2".to_string(),
            reasoning: Some("keyword match".to_string()),
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&record).unwrap();
        let round_tripped: CacheRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, round_tripped);
    }

    #[test]
    fn user_label_record_clear_is_distinct_from_absent_parts() {
        let clear = UserLabelRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            source: "comdirect".to_string(),
            external_id: "ACC1-SPEND-00".to_string(),
            category_slug: None,
            parts: Vec::new(),
            revision: Utc::now(),
            note: None,
        };
        let json = serde_json::to_string(&clear).unwrap();
        let round_tripped: UserLabelRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(clear, round_tripped);
        assert!(round_tripped.category_slug.is_none());
        assert!(round_tripped.parts.is_empty());
    }

    #[test]
    fn rule_record_round_trips_with_conditions() {
        let record = RuleRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            name: "Lidl -> groceries".to_string(),
            category_slug: "food.groceries".to_string(),
            conditions: RuleConditions {
                counterparty_key: Some("lidl".to_string()),
                ..Default::default()
            },
            priority: 0,
            state: RuleState::Active,
            origin: RuleOrigin::Learned,
            auto_approved: true,
            user_touched: false,
            confidence: Some(1.0),
            evidence: None,
            created_at: Utc::now(),
            revision: Utc::now(),
        };

        let json = serde_json::to_string(&record).unwrap();
        let round_tripped: RuleRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, round_tripped);
    }

    #[test]
    fn category_record_round_trips_through_json() {
        let record = CategoryRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: category_uuid("food.groceries"),
            slug: "food.groceries".to_string(),
            parent_slug: Some("food".to_string()),
            name: "Groceries".to_string(),
            kind: CategoryKind::Expense,
            depth: 2,
            sort_order: 0,
            archived: false,
            origin: CategoryOrigin::Seed,
            owner_user_id: None,
            revision: Utc::now(),
        };

        let json = serde_json::to_string(&record).unwrap();
        let round_tripped: CategoryRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, round_tripped);
    }

    #[test]
    fn label_request_target_variants_round_trip() {
        let txn_request = LabelRequestRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            target: LabelRequestTarget::Transaction {
                source: "comdirect".to_string(),
                external_id: "ACC1-SPEND-00".to_string(),
            },
            requested_at: Utc::now(),
        };
        let json = serde_json::to_string(&txn_request).unwrap();
        let round_tripped: LabelRequestRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(txn_request, round_tripped);

        let rule_request = LabelRequestRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            target: LabelRequestTarget::Rule {
                rule_id: Uuid::new_v4(),
            },
            requested_at: Utc::now(),
        };
        let json = serde_json::to_string(&rule_request).unwrap();
        let round_tripped: LabelRequestRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(rule_request, round_tripped);
    }
}
