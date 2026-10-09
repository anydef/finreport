//! Transaction links, the pure half: how much of an expense a set of
//! offsetting transactions actually covers.
//!
//! A link ([`crate::kafka::links`]) names members and their roles and nothing
//! else; this module derives every figure from the members' *own* amounts,
//! so a link cannot go stale when a transaction is corrected. It has no I/O.
//!
//! The model is one expense side and one offsetting side, each of any size:
//!
//! - one bill repaid in two instalments: one expense, two offsets;
//! - one transfer covering three bills: three expenses, one offset;
//! - the common case is a pair.
//!
//! **Allocation.** The money that offsets is `min(offset total, expense
//! total)`: you cannot be reimbursed for more than you paid. It is shared
//! between the members of each side in proportion to their magnitude, rounded
//! to the 4 decimal places money is stored at, with the rounding remainder
//! handed out so the shares always add up to the total exactly and no share
//! exceeds its own transaction.
//!
//! **Over-reimbursement.** If more came back than was paid, the expense is
//! fully offset (its net is zero, never negative) and the excess is reported
//! as `surplus`, attributed to the offsetting members as their
//! `remaining`. The excess is *not* netted against anything else: it is just
//! money that came in. See `graphql::links` for how the income filter treats
//! it.
//!
//! **A member that no longer exists** (the transaction was removed, or lives
//! in an account the caller cannot see) takes no part in the arithmetic. If
//! that empties a side the link is [`LinkStatus::Incomplete`] and offsets
//! nothing, rather than guessing.

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use uuid::Uuid;

use crate::kafka::links::{LinkKind, LinkRole};

/// Money is stored at 4 decimal places (`NUMERIC(20,4)`).
const SCALE: u32 = 4;

/// One member as the arithmetic sees it. `amount` is the transaction's signed
/// amount, or `None` when the transaction could not be found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub transaction_id: Uuid,
    pub role: LinkRole,
    pub amount: Option<Decimal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStatus {
    /// The offsetting side exactly covers the expense side.
    Full,
    /// Something came back, but less than was paid.
    Partial,
    /// More came back than was paid; see [`Offsets::surplus`].
    Over,
    /// A side has no (non-zero) member present: nothing is offset.
    Incomplete,
}

/// One member's share of the offset. All figures are magnitudes (>= 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberShare {
    pub transaction_id: Uuid,
    pub role: LinkRole,
    /// `None` when the transaction is missing.
    pub magnitude: Option<Decimal>,
    /// For an expense: how much of it is offset. For an offset: how much of
    /// it goes to offsetting.
    pub allocated: Decimal,
    /// For an expense: still not covered. For an offset: the part that
    /// offsets nothing (over-reimbursement).
    pub remaining: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offsets {
    /// Sum of the present expense members' magnitudes.
    pub expense_total: Decimal,
    /// Sum of the present offsetting members' magnitudes.
    pub offset_total: Decimal,
    /// `min(offset_total, expense_total)`, or zero when incomplete.
    pub reimbursed: Decimal,
    /// What the expense still costs: `expense_total - reimbursed`.
    pub net: Decimal,
    /// `offset_total - expense_total` when positive, else zero.
    pub surplus: Decimal,
    pub status: LinkStatus,
    /// How many members could not be found.
    pub missing: usize,
    /// In the order the members were given.
    pub shares: Vec<MemberShare>,
}

/// Whether `amount` has the sign `role` requires in a link of `kind`.
///
/// A reimbursement repays money that left (an expense is negative) with money
/// that came in (the offset is positive). A zero amount fits neither.
pub fn amount_fits_role(kind: LinkKind, role: LinkRole, amount: Decimal) -> bool {
    match (kind, role) {
        (LinkKind::Reimbursement, LinkRole::Expense) => amount < Decimal::ZERO,
        (LinkKind::Reimbursement, LinkRole::Offset) => amount > Decimal::ZERO,
    }
}

/// Splits `total` across `weights` in proportion, at [`SCALE`] decimal places,
/// so the parts add up to `total` exactly. Each part is at most its weight
/// whenever `total <= sum(weights)` and the weights are themselves at
/// [`SCALE`] places. All zero when there is nothing to split or to split by.
fn allocate(total: Decimal, weights: &[Decimal]) -> Vec<Decimal> {
    let sum: Decimal = weights.iter().sum();
    if sum.is_zero() || total.is_zero() {
        return vec![Decimal::ZERO; weights.len()];
    }
    let exact: Vec<Decimal> = weights.iter().map(|w| total * *w / sum).collect();
    let mut parts: Vec<Decimal> = exact
        .iter()
        .map(|x| x.round_dp_with_strategy(SCALE, RoundingStrategy::ToZero))
        .collect();
    let unit = Decimal::new(1, SCALE);
    let leftover = total - parts.iter().sum::<Decimal>();
    // Whole units of the last decimal place still to hand out. Largest
    // fractional remainder first, ties to the earlier member, so the result
    // is deterministic.
    let mut units = (leftover / unit).round().to_i64().unwrap_or(0);
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|&a, &b| {
        let fa = exact[a] - parts[a];
        let fb = exact[b] - parts[b];
        fb.cmp(&fa).then(a.cmp(&b))
    });
    for i in order {
        if units <= 0 {
            break;
        }
        if exact[i] > parts[i] {
            parts[i] += unit;
            units -= 1;
        }
    }
    parts
}

/// Derives the figures for one link from its members.
pub fn compute(slots: &[Slot]) -> Offsets {
    let magnitude = |s: &Slot| s.amount.map(|a| a.abs());
    let side = |role: LinkRole| -> Vec<&Slot> { slots.iter().filter(|s| s.role == role).collect() };
    let total = |role: LinkRole| -> Decimal { side(role).iter().filter_map(|s| magnitude(s)).sum() };

    let expense_total = total(LinkRole::Expense);
    let offset_total = total(LinkRole::Offset);
    let missing = slots.iter().filter(|s| s.amount.is_none()).count();

    let complete = !expense_total.is_zero() && !offset_total.is_zero();
    let reimbursed = if complete { expense_total.min(offset_total) } else { Decimal::ZERO };
    let surplus = if complete { (offset_total - expense_total).max(Decimal::ZERO) } else { Decimal::ZERO };
    let status = if !complete {
        LinkStatus::Incomplete
    } else if offset_total == expense_total {
        LinkStatus::Full
    } else if offset_total < expense_total {
        LinkStatus::Partial
    } else {
        LinkStatus::Over
    };

    let mut allocated: std::collections::HashMap<Uuid, Decimal> = std::collections::HashMap::new();
    for role in [LinkRole::Expense, LinkRole::Offset] {
        let present: Vec<&Slot> = side(role).into_iter().filter(|s| s.amount.is_some()).collect();
        let weights: Vec<Decimal> = present.iter().filter_map(|s| magnitude(s)).collect();
        for (slot, part) in present.iter().zip(allocate(reimbursed, &weights)) {
            allocated.insert(slot.transaction_id, part);
        }
    }

    let shares = slots
        .iter()
        .map(|s| {
            let magnitude = magnitude(s);
            let allocated = allocated.get(&s.transaction_id).copied().unwrap_or(Decimal::ZERO);
            MemberShare {
                transaction_id: s.transaction_id,
                role: s.role,
                magnitude,
                allocated,
                remaining: magnitude.map(|m| m - allocated).unwrap_or(Decimal::ZERO),
            }
        })
        .collect();

    Offsets {
        expense_total,
        offset_total,
        reimbursed,
        net: expense_total - reimbursed,
        surplus,
        status,
        missing,
        shares,
    }
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

    fn slot(n: u8, role: LinkRole, amount: &str) -> Slot {
        Slot { transaction_id: id(n), role, amount: Some(d(amount)) }
    }

    fn share(o: &Offsets, n: u8) -> &MemberShare {
        o.shares.iter().find(|s| s.transaction_id == id(n)).unwrap()
    }

    #[test]
    fn a_full_reimbursement_nets_the_expense_to_zero() {
        let o = compute(&[slot(1, LinkRole::Expense, "-1000"), slot(2, LinkRole::Offset, "1000")]);
        assert_eq!(o.status, LinkStatus::Full);
        assert_eq!((o.expense_total, o.offset_total), (d("1000"), d("1000")));
        assert_eq!((o.reimbursed, o.net, o.surplus), (d("1000"), d("0"), d("0")));
        assert_eq!((share(&o, 1).allocated, share(&o, 1).remaining), (d("1000"), d("0")));
        assert_eq!((share(&o, 2).allocated, share(&o, 2).remaining), (d("1000"), d("0")));
    }

    #[test]
    fn a_partial_reimbursement_leaves_the_rest_as_net_cost() {
        let o = compute(&[slot(1, LinkRole::Expense, "-1000"), slot(2, LinkRole::Offset, "600")]);
        assert_eq!(o.status, LinkStatus::Partial);
        assert_eq!((o.reimbursed, o.net, o.surplus), (d("600"), d("400"), d("0")));
        assert_eq!(share(&o, 1).remaining, d("400"));
        assert_eq!(share(&o, 2).allocated, d("600"));
    }

    #[test]
    fn over_reimbursement_caps_at_the_expense_and_reports_the_excess() {
        let o = compute(&[slot(1, LinkRole::Expense, "-1000"), slot(2, LinkRole::Offset, "1200")]);
        assert_eq!(o.status, LinkStatus::Over);
        assert_eq!((o.reimbursed, o.net, o.surplus), (d("1000"), d("0"), d("200")));
        // The expense is fully covered and never goes negative...
        assert_eq!((share(&o, 1).allocated, share(&o, 1).remaining), (d("1000"), d("0")));
        // ...and the excess sits on the offsetting side.
        assert_eq!((share(&o, 2).allocated, share(&o, 2).remaining), (d("1000"), d("200")));
    }

    #[test]
    fn one_expense_offset_by_several_reimbursements_sums_them() {
        let o = compute(&[
            slot(1, LinkRole::Expense, "-900"),
            slot(2, LinkRole::Offset, "300"),
            slot(3, LinkRole::Offset, "300"),
        ]);
        assert_eq!(o.status, LinkStatus::Partial);
        assert_eq!((o.reimbursed, o.net), (d("600"), d("300")));
        assert_eq!(share(&o, 2).allocated, d("300"));
        assert_eq!(share(&o, 3).allocated, d("300"));
    }

    #[test]
    fn one_reimbursement_spread_across_several_expenses_is_shared_by_size() {
        let o = compute(&[
            slot(1, LinkRole::Expense, "-100"),
            slot(2, LinkRole::Expense, "-300"),
            slot(3, LinkRole::Offset, "200"),
        ]);
        assert_eq!(o.status, LinkStatus::Partial);
        assert_eq!((o.reimbursed, o.net), (d("200"), d("200")));
        assert_eq!(share(&o, 1).allocated, d("50"));
        assert_eq!(share(&o, 2).allocated, d("150"));
        assert_eq!(share(&o, 1).remaining + share(&o, 2).remaining, o.net);
    }

    #[test]
    fn a_member_that_no_longer_exists_is_left_out_of_the_arithmetic() {
        let gone = Slot { transaction_id: id(9), role: LinkRole::Expense, amount: None };
        let o = compute(&[slot(1, LinkRole::Expense, "-100"), gone, slot(2, LinkRole::Offset, "100")]);
        assert_eq!(o.missing, 1);
        assert_eq!(o.status, LinkStatus::Full);
        assert_eq!(o.expense_total, d("100"));
        let missing = share(&o, 9);
        assert_eq!((missing.magnitude, missing.allocated, missing.remaining), (None, d("0"), d("0")));
    }

    #[test]
    fn a_side_with_no_member_left_is_incomplete_and_offsets_nothing() {
        let gone = Slot { transaction_id: id(2), role: LinkRole::Offset, amount: None };
        let o = compute(&[slot(1, LinkRole::Expense, "-100"), gone]);
        assert_eq!(o.status, LinkStatus::Incomplete);
        assert_eq!((o.reimbursed, o.net, o.surplus), (d("0"), d("100"), d("0")));
        assert_eq!(share(&o, 1).allocated, d("0"));
        assert_eq!(share(&o, 1).remaining, d("100"));
    }

    #[test]
    fn a_link_with_no_members_at_all_is_incomplete() {
        let o = compute(&[]);
        assert_eq!(o.status, LinkStatus::Incomplete);
        assert_eq!(o.expense_total, d("0"));
    }

    #[test]
    fn shares_add_up_exactly_even_when_the_split_does_not_divide() {
        let o = compute(&[
            slot(1, LinkRole::Expense, "-100"),
            slot(2, LinkRole::Expense, "-100"),
            slot(3, LinkRole::Expense, "-100"),
            slot(4, LinkRole::Offset, "100"),
        ]);
        let parts: Vec<Decimal> = (1..=3).map(|n| share(&o, n).allocated).collect();
        assert_eq!(parts.iter().sum::<Decimal>(), d("100"));
        assert_eq!(parts, vec![d("33.3334"), d("33.3333"), d("33.3333")]);
    }

    #[test]
    fn no_share_ever_exceeds_its_own_transaction() {
        // Awkward sizes and a partial total, to stress the rounding.
        let o = compute(&[
            slot(1, LinkRole::Expense, "-0.0003"),
            slot(2, LinkRole::Expense, "-7.1234"),
            slot(3, LinkRole::Expense, "-91.9999"),
            slot(4, LinkRole::Offset, "50.5051"),
        ]);
        let mut sum = Decimal::ZERO;
        for s in o.shares.iter().filter(|s| s.role == LinkRole::Expense) {
            assert!(s.allocated <= s.magnitude.unwrap(), "{s:?}");
            assert!(s.remaining >= Decimal::ZERO);
            sum += s.allocated;
        }
        assert_eq!(sum, o.reimbursed);
    }

    #[test]
    fn the_sign_rule_for_a_reimbursement() {
        let k = LinkKind::Reimbursement;
        assert!(amount_fits_role(k, LinkRole::Expense, d("-5")));
        assert!(!amount_fits_role(k, LinkRole::Expense, d("5")));
        assert!(amount_fits_role(k, LinkRole::Offset, d("5")));
        assert!(!amount_fits_role(k, LinkRole::Offset, d("-5")));
        assert!(!amount_fits_role(k, LinkRole::Offset, d("0")));
    }
}
