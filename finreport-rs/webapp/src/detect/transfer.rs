//! Internal-transfer matching (iteration 3 §3.1): a greedy, total,
//! deterministic 1:1 pairing of opposite-amount legs between accounts the
//! same user owns.

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

/// One ranked candidate pair before the greedy walk picks winners (§3.1).
struct Candidate {
    a_idx: usize,
    b_idx: usize,
    iban_confirmed: bool,
    date_distance: i64,
}

/// Greedy, total, deterministic 1:1 matching over `txns` (§3.1).
///
/// Builds every candidate pair satisfying the four hard rules, ranks them
/// (`iban_confirmed` first, then smaller date distance, then `(min(id),
/// max(id))` ascending — §3.1), then walks that order taking a pair only if
/// neither side is already matched.
pub fn detect_transfers(txns: &[TxnFacts], cfg: TransferConfig) -> Vec<TransferPair> {
    let mut candidates: Vec<Candidate> = Vec::new();

    for i in 0..txns.len() {
        for j in (i + 1)..txns.len() {
            let a = &txns[i];
            let b = &txns[j];

            // (1) different accounts.
            if a.account_id == b.account_id {
                continue;
            }
            // (2) at least one shared owner.
            if !a.owner_user_ids.iter().any(|u| b.owner_user_ids.contains(u)) {
                continue;
            }
            // (3) exact opposite amount, non-zero.
            if a.amount == rust_decimal::Decimal::ZERO || a.amount != -b.amount {
                continue;
            }
            // (4) booking-date window.
            let date_distance = (a.booking_date - b.booking_date).num_days().abs();
            if date_distance > i64::from(cfg.match_days) {
                continue;
            }

            let iban_confirmed = a
                .counterparty_iban
                .as_deref()
                .is_some_and(|iban| Some(iban) == b.account_iban.as_deref())
                || b.counterparty_iban
                    .as_deref()
                    .is_some_and(|iban| Some(iban) == a.account_iban.as_deref());

            candidates.push(Candidate {
                a_idx: i,
                b_idx: j,
                iban_confirmed,
                date_distance,
            });
        }
    }

    candidates.sort_by(|x, y| {
        let xa = &txns[x.a_idx];
        let xb = &txns[x.b_idx];
        let ya = &txns[y.a_idx];
        let yb = &txns[y.b_idx];
        y.iban_confirmed
            .cmp(&x.iban_confirmed)
            .then(x.date_distance.cmp(&y.date_distance))
            .then(xa.id.min(xb.id).cmp(&ya.id.min(yb.id)))
            .then(xa.id.max(xb.id).cmp(&ya.id.max(yb.id)))
    });

    let mut matched = std::collections::HashSet::new();
    let mut pairs = Vec::new();
    for c in candidates {
        let a = &txns[c.a_idx];
        let b = &txns[c.b_idx];
        if matched.contains(&a.id) || matched.contains(&b.id) {
            continue;
        }
        matched.insert(a.id);
        matched.insert(b.id);
        pairs.push(TransferPair {
            a: a.id,
            b: b.id,
            match_kind: if c.iban_confirmed {
                TransferMatchKind::Iban
            } else {
                TransferMatchKind::AmountDate
            },
        });
    }

    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn amt(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn cfg() -> TransferConfig {
        TransferConfig { match_days: 3 }
    }

    fn txn(
        id: Uuid,
        account_id: Uuid,
        owners: &[Uuid],
        date: NaiveDate,
        amount: Decimal,
    ) -> TxnFacts {
        TxnFacts {
            id,
            account_id,
            source: "comdirect".to_string(),
            external_id: id.to_string(),
            owner_user_ids: owners.to_vec(),
            booking_date: date,
            amount,
            counterparty_iban: None,
            account_iban: None,
            counterparty_key: None,
        }
    }

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 7, day).unwrap()
    }

    #[test]
    fn stub_returns_no_pairs_when_empty() {
        assert_eq!(detect_transfers(&[], cfg()), Vec::new());
    }

    #[test]
    fn exact_amount_only_no_fee_tolerance() {
        let user = Uuid::new_v4();
        let acc_a = Uuid::new_v4();
        let acc_b = Uuid::new_v4();
        let a = txn(Uuid::new_v4(), acc_a, &[user], d(15), amt("-100.00"));
        let b = txn(Uuid::new_v4(), acc_b, &[user], d(15), amt("99.95"));
        assert!(detect_transfers(&[a, b], cfg()).is_empty());
    }

    #[test]
    fn window_boundary_at_exactly_n_days_matches() {
        let user = Uuid::new_v4();
        let acc_a = Uuid::new_v4();
        let acc_b = Uuid::new_v4();
        let a = txn(Uuid::new_v4(), acc_a, &[user], d(10), amt("-50.00"));
        let b = txn(Uuid::new_v4(), acc_b, &[user], d(13), amt("50.00"));
        let pairs = detect_transfers(&[a, b], cfg());
        assert_eq!(pairs.len(), 1);
    }

    #[test]
    fn window_boundary_one_day_past_n_rejects() {
        let user = Uuid::new_v4();
        let acc_a = Uuid::new_v4();
        let acc_b = Uuid::new_v4();
        let a = txn(Uuid::new_v4(), acc_a, &[user], d(10), amt("-50.00"));
        let b = txn(Uuid::new_v4(), acc_b, &[user], d(14), amt("50.00"));
        assert!(detect_transfers(&[a, b], cfg()).is_empty());
    }

    #[test]
    fn same_account_rejected() {
        let user = Uuid::new_v4();
        let acc = Uuid::new_v4();
        let a = txn(Uuid::new_v4(), acc, &[user], d(15), amt("-50.00"));
        let b = txn(Uuid::new_v4(), acc, &[user], d(15), amt("50.00"));
        assert!(detect_transfers(&[a, b], cfg()).is_empty());
    }

    #[test]
    fn no_common_owner_rejected() {
        let acc_a = Uuid::new_v4();
        let acc_b = Uuid::new_v4();
        let a = txn(Uuid::new_v4(), acc_a, &[Uuid::new_v4()], d(15), amt("-50.00"));
        let b = txn(Uuid::new_v4(), acc_b, &[Uuid::new_v4()], d(15), amt("50.00"));
        assert!(detect_transfers(&[a, b], cfg()).is_empty());
    }

    #[test]
    fn shared_family_account_one_common_owner_accepted() {
        let u1 = Uuid::new_v4();
        let u2 = Uuid::new_v4();
        let shared = Uuid::new_v4();
        let acc_a = Uuid::new_v4();
        let acc_b = Uuid::new_v4();
        let a = txn(Uuid::new_v4(), acc_a, &[u1, shared], d(15), amt("-50.00"));
        let b = txn(Uuid::new_v4(), acc_b, &[u2, shared], d(15), amt("50.00"));
        assert_eq!(detect_transfers(&[a, b], cfg()).len(), 1);
    }

    #[test]
    fn iban_confirmed_outranks_amount_only() {
        let user = Uuid::new_v4();
        let acc_a = Uuid::new_v4();
        let acc_b = Uuid::new_v4();
        let acc_c = Uuid::new_v4();
        // `a` could match either `b` (amount-only, same day) or `c`
        // (iban-confirmed, one day further) — iban must win despite the
        // worse date distance.
        let a_id = Uuid::new_v4();
        let b_id = Uuid::new_v4();
        let c_id = Uuid::new_v4();
        let mut a = txn(a_id, acc_a, &[user], d(15), amt("-50.00"));
        let b = txn(b_id, acc_b, &[user], d(15), amt("50.00"));
        let mut c = txn(c_id, acc_c, &[user], d(16), amt("50.00"));
        a.counterparty_iban = Some("DE_C".to_string());
        c.account_iban = Some("DE_C".to_string());
        let pairs = detect_transfers(&[a, b, c], cfg());
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].match_kind, TransferMatchKind::Iban);
        assert!(pairs[0].a == a_id || pairs[0].a == c_id);
        assert!(pairs[0].b == a_id || pairs[0].b == c_id);
    }

    #[test]
    fn two_identical_candidate_pairs_match_1_to_1_under_reordering() {
        let user = Uuid::new_v4();
        let acc_a = Uuid::new_v4();
        let acc_b = Uuid::new_v4();
        let a1 = txn(Uuid::new_v4(), acc_a, &[user], d(15), amt("-50.00"));
        let a2 = txn(Uuid::new_v4(), acc_a, &[user], d(15), amt("-50.00"));
        let b1 = txn(Uuid::new_v4(), acc_b, &[user], d(15), amt("50.00"));
        let b2 = txn(Uuid::new_v4(), acc_b, &[user], d(15), amt("50.00"));

        let forward = detect_transfers(&[a1.clone(), a2.clone(), b1.clone(), b2.clone()], cfg());
        let reordered = detect_transfers(&[b2, a1, b1, a2], cfg());

        assert_eq!(forward.len(), 2);
        assert_eq!(reordered.len(), 2);
        let mut forward_ids: Vec<Uuid> = forward.iter().flat_map(|p| [p.a, p.b]).collect();
        let mut reordered_ids: Vec<Uuid> = reordered.iter().flat_map(|p| [p.a, p.b]).collect();
        forward_ids.sort();
        reordered_ids.sort();
        assert_eq!(forward_ids, reordered_ids);
    }

    #[test]
    fn zero_amount_rejected() {
        let user = Uuid::new_v4();
        let acc_a = Uuid::new_v4();
        let acc_b = Uuid::new_v4();
        let a = txn(Uuid::new_v4(), acc_a, &[user], d(15), amt("0.00"));
        let b = txn(Uuid::new_v4(), acc_b, &[user], d(15), amt("0.00"));
        assert!(detect_transfers(&[a, b], cfg()).is_empty());
    }

    #[test]
    fn partial_match_fee_difference_no_match() {
        let user = Uuid::new_v4();
        let acc_a = Uuid::new_v4();
        let acc_b = Uuid::new_v4();
        let a = txn(Uuid::new_v4(), acc_a, &[user], d(15), amt("-100.00"));
        let b = txn(Uuid::new_v4(), acc_b, &[user], d(15), amt("98.50"));
        assert!(detect_transfers(&[a, b], cfg()).is_empty());
    }
}
