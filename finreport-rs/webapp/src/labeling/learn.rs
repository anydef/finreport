//! §2.8 rule learning: promote a repeated `counterparty_key` → `category`
//! observation into a learned `rule` record once it crosses
//! `APP_rule_learn_min_observations` (default 3) with high enough confidence,
//! without ever overwriting a user-touched rule.
//!
//! WP0 stub — owned by WP2.

/// One observed (`counterparty_key`, resolved category) pair the learner is
/// considering promoting to a rule.
#[derive(Debug, Clone)]
pub struct Observation<'a> {
    pub counterparty_key: &'a str,
    pub category_slug: &'a str,
    /// `1.0` for a user-confirmed label (§2.8), the LLM's own confidence
    /// otherwise.
    pub confidence: f32,
}

/// A candidate learned rule, not yet published.
#[derive(Debug, Clone, PartialEq)]
pub struct LearnedRule {
    pub counterparty_key: String,
    pub category_slug: String,
    pub confidence: f32,
    /// `true` at/above `APP_rule_auto_approve_threshold` (default `0.9`,
    /// inclusive — the exact boundary §8 calls out).
    pub auto_approved: bool,
}

/// Decides whether `observations` (all sharing one `counterparty_key`) should
/// become a learned rule, and whether it auto-approves.
///
/// TODO(WP2): implement per §2.8; unit-test the N-1-vs-N-observations
/// boundary, a conflicting observation disqualifying the group, the
/// min-confidence gate, user-confirmed = 1.0, the auto-approve threshold at
/// exactly 0.9, and a revoked rule never being re-learned (§8 "Learner" row).
pub fn consider(_observations: &[Observation<'_>]) -> Option<LearnedRule> {
    todo!("WP2: §2.8 rule learning — observation threshold, conflict/confidence gates, auto-approve boundary")
}
