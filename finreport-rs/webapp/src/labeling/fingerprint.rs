//! §2.5 fingerprint: `sha256(provider_id ‖ model ‖ prompt_version ‖
//! normalized_counterparty ‖ normalized_description ‖ direction)`. Amount is
//! deliberately excluded — the same merchant at a different price is the
//! same answer. This is the `llm_label_cache` primary key and part of
//! `transaction_label.fingerprint`, which the labeler's compare-before-publish
//! rule (§2.3) matches against to decide whether a stored LLM label is still
//! valid.
//!
//! WP0 stub — owned by WP2.

/// A transaction's direction, for the fingerprint only — not the richer
/// `category.kind` (income/expense/transfer/saving).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Credit,
    Debit,
}

/// Computes the §2.5 fingerprint.
///
/// TODO(WP2): implement per §2.5; unit-test stability across amounts and
/// sensitivity to provider/model/prompt-version changes (§8 "Fingerprint"
/// row).
pub fn fingerprint(
    _provider_id: &str,
    _model: &str,
    _prompt_version: &str,
    _normalized_counterparty: &str,
    _normalized_description: &str,
    _direction: Direction,
) -> String {
    todo!("WP2: §2.5 fingerprint — sha256 of provider/model/prompt_version/normalized fields/direction")
}
