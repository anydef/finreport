//! Internal-transfer matching (iteration 3 §3.1). **WP-A owns the real
//! body** of [`detect_transfers`]; WP0 only freezes the signature so the
//! workspace compiles and [`crate::projection`]/[`crate::detect::processor`]
//! can wire against it.

use super::TxnFacts;
use uuid::Uuid;

/// Tunables for [`detect_transfers`] (iteration 3 §2.4).
#[derive(Debug, Clone, Copy)]
pub struct TransferConfig {
    /// `APP_transfer_match_days`: inclusive window on
    /// `abs(Δ booking_date)` between the two candidate legs.
    pub match_days: u32,
}

/// How a [`TransferPair`] was matched (§3.1 ranking buckets), mirrors the
/// `transfer_match` column / `TransferMatch` GraphQL enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMatchKind {
    /// One side's `counterparty_iban` equals the other side's account IBAN.
    Iban,
    /// Matched on exact opposite amount + date proximity alone.
    AmountDate,
}

/// A matched pair of transfer legs (§3.1): both sides get `is_transfer =
/// true` plus each other's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPair {
    pub a: Uuid,
    pub b: Uuid,
    pub match_kind: TransferMatchKind,
}

/// Greedy, total, deterministic 1:1 matching over `txns` (§3.1). **Stub**:
/// WP0 freezes the signature only — always returns `vec![]` until WP-A
/// implements the candidate-ranking walk.
pub fn detect_transfers(_txns: &[TxnFacts], _cfg: TransferConfig) -> Vec<TransferPair> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_returns_no_pairs() {
        let cfg = TransferConfig { match_days: 3 };
        assert_eq!(detect_transfers(&[], cfg), Vec::new());
    }
}
