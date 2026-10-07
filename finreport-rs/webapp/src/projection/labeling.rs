//! Projections for the five new §2.2 labeling topics
//! (`transaction-label`, `llm-cache`, `user-label`, `rule`, `category`) into
//! the §3 tables (`transaction_label`, `llm_label_cache`,
//! `transaction_user_label`/`transaction_split`, `rule`, `category`).
//!
//! **Stub.** WP0 only freezes the dispatch entry point
//! (`webapp/src/projection/mod.rs`'s `labeling_topics`/
//! `dispatch_labeling_record`) so the workspace compiles while WP3 — which
//! owns this file — builds the real four-topic consume → normalize →
//! resolve → compare → publish loop (§2.3) plus the tombstone/`revision`/
//! split-mismatch handling described in §3's "No foreign keys on these five
//! tables" note. None of this is wired into the live `projector` consumer
//! loop; the forthcoming `labeler` binary (also WP3) is the only caller.

use sea_orm::{DatabaseTransaction, DbErr};

use crate::kafka::labeling::{CategoryRecord, LabelRecord, RuleRecord, UserLabelRecord};

/// Upserts (or, on `None`, tombstone-deletes) one `category` row.
///
/// TODO(WP3): implement per §3's `category` table + the idempotent
/// `category-seed` bin's publish contract.
pub async fn project_category(
    _txn: &DatabaseTransaction,
    _category_id: uuid::Uuid,
    _record: Option<CategoryRecord>,
) -> Result<(), DbErr> {
    todo!("WP3: project finreport.category into the category table (§3)")
}

/// Upserts (or tombstone-deletes) one `transaction_label` row.
///
/// TODO(WP3): implement per §2.4/§2.5, including the compare-before-publish
/// semantics described in §2.3 (a label is only ever re-published when its
/// *inputs* changed, never purely because its textual shape differs).
pub async fn project_transaction_label(
    _txn: &DatabaseTransaction,
    _key: &str,
    _record: Option<LabelRecord>,
) -> Result<(), DbErr> {
    todo!("WP3: project finreport.transaction-label into transaction_label (§2.4/§2.5)")
}

/// Upserts (or tombstone-deletes) one `llm_label_cache` row, keyed by
/// `fingerprint`.
///
/// TODO(WP3): implement per §2.4/§3; must reproduce identical cache contents
/// on a rebuild from offset 0 (§8 "Projection" integration row).
pub async fn project_llm_cache(
    _txn: &DatabaseTransaction,
    _fingerprint: &str,
    _record: Option<crate::kafka::labeling::CacheRecord>,
) -> Result<(), DbErr> {
    todo!("WP3: project finreport.llm-cache into llm_label_cache (§2.4)")
}

/// Upserts (or tombstone-deletes) the `transaction_user_label` row and its
/// `transaction_split` rows for one transaction.
///
/// TODO(WP3): implement per §2.6, including the split-mismatch invalidation
/// described in §2.5's "Edge cases" (`NOTBOOKED` re-published with a
/// different amount marks `transaction_split.invalid = true`).
pub async fn project_user_label(
    _txn: &DatabaseTransaction,
    _key: &str,
    _record: Option<UserLabelRecord>,
) -> Result<(), DbErr> {
    todo!("WP3: project finreport.user-label into transaction_user_label/transaction_split (§2.6)")
}

/// Upserts (or tombstone-deletes) one `rule` row.
///
/// TODO(WP3): implement per §2.7/§2.8, honouring `user_touched` so the
/// learner never overwrites a user-edited rule.
pub async fn project_rule(
    _txn: &DatabaseTransaction,
    _rule_id: uuid::Uuid,
    _record: Option<RuleRecord>,
) -> Result<(), DbErr> {
    todo!("WP3: project finreport.rule into the rule table (§2.7/§2.8)")
}
