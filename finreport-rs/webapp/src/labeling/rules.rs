//! §2.7 rule matching: AND of conditions, most-specific-active-rule-wins
//! ordering with a deterministic tie-break, invalid regex skipped rather than
//! panicking.
//!
//! WP0 stub — owned by WP2.

use crate::kafka::labeling::RuleRecord;

/// A transaction's matchable fields, as the rules engine sees them —
/// deliberately narrower than the full `TransactionRecord` (§2.5's
/// resolution chain only ever needs these).
#[derive(Debug, Clone)]
pub struct RuleMatchInput<'a> {
    pub counterparty_key: Option<&'a str>,
    pub description: Option<&'a str>,
    pub transaction_type: Option<&'a str>,
}

/// Finds the most specific *active* rule among `rules` whose conditions all
/// match `input`, per §2.7's specificity ordering and deterministic
/// tie-break.
///
/// TODO(WP2): implement per §2.7; unit-test the AND of conditions,
/// specificity ordering, the tie-break and invalid-regex-skips-not-panics
/// (§8 "Rule matching" row).
pub fn most_specific_match<'a>(
    _rules: &'a [RuleRecord],
    _input: &RuleMatchInput<'_>,
) -> Option<&'a RuleRecord> {
    todo!("WP2: §2.7 rule matching — AND of conditions, specificity ordering, deterministic tie-break")
}
