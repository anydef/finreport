//! Savings accounts, the pure half: net balance change per savings account.
//!
//! The figure is *net balance change* (decided deliberately, see CLAUDE.md
//! "Savings"): every movement that changes the total held in savings counts -
//! transfers in, withdrawals out, interest or salary paid straight in, a
//! payment made directly from a savings account. A movement **between two
//! savings accounts** changes no total and counts as neither.
//!
//! **No double counting.** The transfer detector flags *both* legs of an
//! internal transfer and records each leg's counterpart. A row is therefore
//! internal exactly when its counterpart lives on a savings account, and it is
//! dropped. Only rows on a savings account are ever fed in, so the savings
//! side of a savings-to-checking transfer is counted once (its checking leg is
//! not a row of ours), and a savings-to-savings transfer is dropped from both
//! sides. [`Netting::internal_transfer_count`] counts *movements*: the two
//! legs share one pair key, so they count once, and so does a movement of
//! which only one leg is inside the period.
//!
//! **Spent directly** is the part of `withdrawn` that left to an outside
//! payee: a withdrawal the detector did not flag as a transfer. It is also in
//! the spending card, which excludes transfers; the UI discloses the overlap.
//!
//! No I/O; see `graphql::savings` for the single query that feeds this.

use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// One transaction on a savings account, as the arithmetic sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Movement {
    pub transaction_id: Uuid,
    pub account_id: Uuid,
    /// Signed, as on the transaction.
    pub amount: Decimal,
    /// The detector's `is_transfer` flag for this row.
    pub is_transfer: bool,
    /// The other leg of a detected transfer, and the account it sits on.
    pub counterpart_id: Option<Uuid>,
    pub counterpart_account_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountFlow {
    pub account_id: Uuid,
    pub paid_in: Decimal,
    /// A positive magnitude.
    pub withdrawn: Decimal,
}

impl AccountFlow {
    pub fn net(&self) -> Decimal {
        self.paid_in - self.withdrawn
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Netting {
    /// One entry per requested account, in the order given, zeroes included.
    pub accounts: Vec<AccountFlow>,
    pub paid_in: Decimal,
    pub withdrawn: Decimal,
    /// Of `withdrawn`, what went to an outside payee.
    pub spent_directly: Decimal,
    /// Savings-to-savings movements excluded, one per movement.
    pub internal_transfer_count: usize,
}

impl Netting {
    pub fn net(&self) -> Decimal {
        self.paid_in - self.withdrawn
    }
}

/// Nets `movements` (rows of `accounts`) into per-account and total flows.
/// `all_savings` is every account the caller flagged - not just `accounts` -
/// so a counterpart on a flagged account outside the panel's selection is
/// still recognised as internal.
pub fn net_movements(
    accounts: &[Uuid],
    all_savings: &HashSet<Uuid>,
    movements: &[Movement],
) -> Netting {
    let mut flows: Vec<AccountFlow> = accounts
        .iter()
        .map(|&account_id| AccountFlow { account_id, paid_in: Decimal::ZERO, withdrawn: Decimal::ZERO })
        .collect();
    let index: HashMap<Uuid, usize> =
        accounts.iter().enumerate().map(|(i, &a)| (a, i)).collect();
    let mut internal: HashSet<(Uuid, Uuid)> = HashSet::new();
    let mut spent_directly = Decimal::ZERO;

    for m in movements {
        let Some(&slot) = index.get(&m.account_id) else { continue };
        if m.counterpart_account_id.is_some_and(|a| all_savings.contains(&a)) {
            // The other leg is on a savings account too: no total changes.
            let other = m.counterpart_id.unwrap_or(m.transaction_id);
            internal.insert((m.transaction_id.min(other), m.transaction_id.max(other)));
            continue;
        }
        if m.amount > Decimal::ZERO {
            flows[slot].paid_in += m.amount;
        } else if m.amount < Decimal::ZERO {
            let out = -m.amount;
            flows[slot].withdrawn += out;
            // Left for another of the user's own accounts: not spending.
            if !m.is_transfer && m.counterpart_account_id.is_none() {
                spent_directly += out;
            }
        }
    }

    let paid_in = flows.iter().map(|f| f.paid_in).sum();
    let withdrawn = flows.iter().map(|f| f.withdrawn).sum();
    Netting { accounts: flows, paid_in, withdrawn, spent_directly, internal_transfer_count: internal.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }
    fn id(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn plain(tx: u8, account: u8, amount: &str) -> Movement {
        Movement {
            transaction_id: id(tx),
            account_id: id(account),
            amount: d(amount),
            is_transfer: false,
            counterpart_id: None,
            counterpart_account_id: None,
        }
    }

    /// A detected transfer leg towards `(counterpart tx, counterpart account)`.
    fn leg(tx: u8, account: u8, amount: &str, cp_tx: u8, cp_account: u8) -> Movement {
        Movement {
            is_transfer: true,
            counterpart_id: Some(id(cp_tx)),
            counterpart_account_id: Some(id(cp_account)),
            ..plain(tx, account, amount)
        }
    }

    fn savings(accounts: &[u8]) -> HashSet<Uuid> {
        accounts.iter().map(|&a| id(a)).collect()
    }

    #[test]
    fn in_out_and_net_per_account() {
        let (a, b) = (1, 2);
        let rows = [
            plain(10, a, "100"),
            plain(11, a, "-30"),
            plain(12, b, "5.50"),
            plain(13, b, "-20"),
            plain(14, b, "-2.25"),
        ];
        let n = net_movements(&[id(a), id(b)], &savings(&[a, b]), &rows);
        assert_eq!((n.accounts[0].paid_in, n.accounts[0].withdrawn, n.accounts[0].net()), (d("100"), d("30"), d("70")));
        assert_eq!((n.accounts[1].paid_in, n.accounts[1].withdrawn, n.accounts[1].net()), (d("5.50"), d("22.25"), d("-16.75")));
        assert_eq!((n.paid_in, n.withdrawn, n.net()), (d("105.50"), d("52.25"), d("53.25")));
        assert_eq!(n.internal_transfer_count, 0);
    }

    #[test]
    fn a_savings_to_savings_movement_counts_as_neither_and_once() {
        // 20 leaves account 1 (tx 10) and arrives on account 2 (tx 20); both legs are rows.
        let rows = [
            leg(10, 1, "-20", 20, 2),
            leg(20, 2, "20", 10, 1),
            plain(11, 1, "50"),
        ];
        let n = net_movements(&[id(1), id(2)], &savings(&[1, 2]), &rows);
        assert_eq!((n.paid_in, n.withdrawn, n.net()), (d("50"), d("0"), d("50")));
        assert_eq!(n.accounts[1].net(), d("0"));
        assert_eq!(n.internal_transfer_count, 1, "two legs, one movement");
    }

    #[test]
    fn a_movement_with_one_leg_outside_the_period_still_counts_once_and_is_dropped() {
        let rows = [leg(10, 1, "-20", 20, 2)];
        let n = net_movements(&[id(1), id(2)], &savings(&[1, 2]), &rows);
        assert_eq!((n.paid_in, n.withdrawn), (d("0"), d("0")));
        assert_eq!(n.internal_transfer_count, 1);
    }

    #[test]
    fn a_counterpart_on_a_savings_account_outside_the_panel_is_still_internal() {
        // Panel shows only account 1; account 2 is flagged but not selected.
        let rows = [leg(10, 1, "-20", 20, 2)];
        let n = net_movements(&[id(1)], &savings(&[1, 2]), &rows);
        assert_eq!(n.withdrawn, d("0"));
        assert_eq!(n.internal_transfer_count, 1);
    }

    #[test]
    fn a_transfer_to_an_ordinary_account_counts_but_is_not_spent_directly() {
        // Account 9 is the user's checking account (not savings).
        let rows = [leg(10, 1, "-200", 90, 9), leg(11, 1, "75", 91, 9)];
        let n = net_movements(&[id(1)], &savings(&[1]), &rows);
        assert_eq!((n.paid_in, n.withdrawn, n.net()), (d("75"), d("200"), d("-125")));
        assert_eq!(n.spent_directly, d("0"));
        assert_eq!(n.internal_transfer_count, 0);
    }

    #[test]
    fn a_withdrawal_to_an_outside_payee_is_in_withdrawn_and_spent_directly() {
        let rows = [plain(10, 1, "-40"), leg(11, 1, "-60", 90, 9), plain(12, 1, "10")];
        let n = net_movements(&[id(1)], &savings(&[1]), &rows);
        assert_eq!(n.withdrawn, d("100"));
        assert_eq!(n.spent_directly, d("40"));
        assert!(n.spent_directly <= n.withdrawn);
    }

    #[test]
    fn a_period_that_drew_the_account_down_is_negative() {
        let rows = [plain(10, 1, "-300"), plain(11, 1, "100")];
        let n = net_movements(&[id(1)], &savings(&[1]), &rows);
        assert_eq!(n.net(), d("-200"));
        assert_eq!(n.accounts[0].net(), d("-200"));
    }

    #[test]
    fn an_empty_set_yields_zeroes_for_every_account() {
        let n = net_movements(&[id(1), id(2)], &savings(&[1, 2]), &[]);
        assert_eq!(n.accounts.len(), 2);
        assert_eq!((n.paid_in, n.withdrawn, n.spent_directly, n.net()), (d("0"), d("0"), d("0"), d("0")));
        assert_eq!(n.internal_transfer_count, 0);
        assert_eq!(net_movements(&[], &savings(&[]), &[]).accounts.len(), 0);
    }

    #[test]
    fn rows_of_an_account_not_asked_for_are_ignored() {
        let n = net_movements(&[id(1)], &savings(&[1]), &[plain(10, 7, "999")]);
        assert_eq!(n.paid_in, d("0"));
    }
}
