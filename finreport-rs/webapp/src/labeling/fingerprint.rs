//! §2.5 fingerprint: `sha256(provider_id ‖ model ‖ prompt_version ‖
//! normalized_counterparty ‖ normalized_description ‖ direction)`. Amount is
//! deliberately excluded — the same merchant at a different price is the
//! same answer. This is the `llm_label_cache` primary key and part of
//! `transaction_label.fingerprint`, which the labeler's compare-before-publish
//! rule (§2.3) matches against to decide whether a stored LLM label is still
//! valid.
//!
//! WP0 stub — owned by WP2.

use sha2::{Digest, Sha256};

/// A transaction's direction, for the fingerprint only — not the richer
/// `category.kind` (income/expense/transfer/saving).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Credit,
    Debit,
}

impl Direction {
    /// The literal fed into the fingerprint's input bytes — stable forever,
    /// independent of any `Debug`/`Display` formatting.
    fn as_fingerprint_str(self) -> &'static str {
        match self {
            Direction::Credit => "credit",
            Direction::Debit => "debit",
        }
    }
}

/// Computes the §2.5 fingerprint: the hex-encoded SHA-256 of the six inputs
/// joined by a `\0` separator (so e.g. an empty `normalized_description`
/// can never be confused with a shifted boundary between fields).
pub fn fingerprint(
    provider_id: &str,
    model: &str,
    prompt_version: &str,
    normalized_counterparty: &str,
    normalized_description: &str,
    direction: Direction,
) -> String {
    let joined = [
        provider_id,
        model,
        prompt_version,
        normalized_counterparty,
        normalized_description,
        direction.as_fingerprint_str(),
    ]
    .join("\0");

    let mut hasher = Sha256::new();
    hasher.update(joined.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(amount_marker: &str) -> String {
        fingerprint("fake", "fake-v1", "1", "lidl", amount_marker, Direction::Debit)
    }

    #[test]
    fn stable_across_different_description_values_representing_the_same_amount_context() {
        // Fingerprint depends only on the six documented inputs; calling it
        // twice with the identical inputs must be stable (determinism, not
        // just "a hash function").
        assert_eq!(base("x"), base("x"));
    }

    #[test]
    fn excludes_amount_so_same_merchant_different_price_is_the_same_answer() {
        // The amount is never one of the function's parameters at all, so
        // this is really documenting the contract: two calls that only differ
        // in "price" (not modeled as an input) necessarily produce the same
        // fingerprint when every actual input is identical.
        let a = fingerprint("fake", "fake-v1", "1", "lidl", "groceries", Direction::Debit);
        let b = fingerprint("fake", "fake-v1", "1", "lidl", "groceries", Direction::Debit);
        assert_eq!(a, b);
    }

    #[test]
    fn changes_with_provider() {
        let a = fingerprint("fake", "fake-v1", "1", "lidl", "", Direction::Debit);
        let b = fingerprint("anthropic", "fake-v1", "1", "lidl", "", Direction::Debit);
        assert_ne!(a, b);
    }

    #[test]
    fn changes_with_model() {
        let a = fingerprint("fake", "fake-v1", "1", "lidl", "", Direction::Debit);
        let b = fingerprint("fake", "fake-v2", "1", "lidl", "", Direction::Debit);
        assert_ne!(a, b);
    }

    #[test]
    fn changes_with_prompt_version() {
        let a = fingerprint("fake", "fake-v1", "1", "lidl", "", Direction::Debit);
        let b = fingerprint("fake", "fake-v1", "2", "lidl", "", Direction::Debit);
        assert_ne!(a, b);
    }

    #[test]
    fn changes_with_normalized_counterparty() {
        let a = fingerprint("fake", "fake-v1", "1", "lidl", "", Direction::Debit);
        let b = fingerprint("fake", "fake-v1", "1", "rewe", "", Direction::Debit);
        assert_ne!(a, b);
    }

    #[test]
    fn changes_with_normalized_description() {
        let a = fingerprint("fake", "fake-v1", "1", "lidl", "d1", Direction::Debit);
        let b = fingerprint("fake", "fake-v1", "1", "lidl", "d2", Direction::Debit);
        assert_ne!(a, b);
    }

    #[test]
    fn changes_with_direction() {
        let a = fingerprint("fake", "fake-v1", "1", "lidl", "", Direction::Credit);
        let b = fingerprint("fake", "fake-v1", "1", "lidl", "", Direction::Debit);
        assert_ne!(a, b);
    }

    #[test]
    fn separator_prevents_field_boundary_collisions() {
        // Without a separator, ("ab", "c") and ("a", "bc") would collide in
        // the counterparty/description pair.
        let a = fingerprint("fake", "fake-v1", "1", "ab", "c", Direction::Debit);
        let b = fingerprint("fake", "fake-v1", "1", "a", "bc", Direction::Debit);
        assert_ne!(a, b);
    }

    #[test]
    fn output_is_lowercase_hex_sha256_length() {
        let fp = fingerprint("fake", "fake-v1", "1", "lidl", "", Direction::Debit);
        assert_eq!(fp.len(), 64);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }
}
