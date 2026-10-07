//! §2.5 normalization: "lowercase, strip accents, collapse whitespace, drop a
//! trailing legal form (`gmbh`, `ag`, `e.k.`, `kg`, `se`), drop card-terminal
//! noise (`//`, `sagt danke`, trailing store numbers, `kartenzahlung`, a
//! trailing date), falling back to the normalized description when the
//! counterparty is empty."
//!
//! WP0 stub — owned by WP2. The projector (WP3) writes this function's
//! result to `transaction.counterparty_key` (§3); the fake provider (WP0's
//! `categorizer::provider::fake`) and real providers (WP1) consume its
//! output via `LabelRequest::counterparty`, they never call it themselves.

/// Normalizes a counterparty (or, if empty, a description) into the stable
/// key rules, the LLM fingerprint and the learner index on.
///
/// TODO(WP2): implement per §2.5's normalization rules; unit-test legal
/// forms, card-terminal noise, accents and the empty-counterparty fallback
/// (§8 "Normalization" row).
pub fn normalize(_counterparty: Option<&str>, _description: Option<&str>) -> String {
    todo!("WP2: §2.5 normalization — lowercase/accents/legal-form/card-noise stripping")
}
