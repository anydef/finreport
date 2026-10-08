//! What counts toward a goal (iteration 4 §3.3): pure arithmetic over
//! contribution rows.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use uuid::Uuid;

use super::scope::Scope;
use crate::kafka::goals::GoalType;

/// One contribution row (§3.1): a whole unsplit transaction, or one part of a
/// split. Transfers have already been dropped by the row query
/// (`transfer_exclusion_sql`).
#[derive(Debug, Clone, PartialEq)]
pub struct ContributionRow {
    pub transaction_id: Uuid,
    /// `None` when the transaction is not split.
    pub part_index: Option<i32>,
    /// Signed, as booked.
    pub amount: Decimal,
    pub category_slug: Option<String>,
    /// `category.kind` of the resolved category.
    pub category_kind: Option<String>,
    /// Tags live on the transaction; every part of a split inherits them.
    pub tags: Vec<String>,
    pub booking_date: NaiveDate,
    /// `transaction_label.status = 'needs_review'`.
    pub held_for_review: bool,
}

/// How a single row lands in a goal's tally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Adds this signed amount (in goal terms) to `total`.
    Counted(Decimal),
    /// Adds this signed amount to `pending`, never to `total`.
    Pending(Decimal),
    Ignored,
}

/// The row's value in goal terms: a limit sums spending as positive
/// magnitude (a refund is negative, reducing the total); a target counts the
/// magnitude whatever the sign.
fn value(goal_type: GoalType, amount: Decimal) -> Decimal {
    match goal_type {
        GoalType::SpendingLimit => -amount,
        GoalType::SavingTarget => amount.abs(),
    }
}

pub fn classify(goal_type: GoalType, scope: &Scope, row: &ContributionRow) -> Outcome {
    // Defence in depth next to the SQL transfer exclusion: a row whose
    // category is a transfer never counts either (the cashflow outcome query
    // drops these as well).
    if row.category_kind.as_deref() == Some("transfer") {
        return Outcome::Ignored;
    }

    if scope.matches(row.category_slug.as_deref(), &row.tags) {
        if goal_type == GoalType::SavingTarget && row.category_kind.as_deref() != Some("saving") {
            return Outcome::Ignored;
        }
        let v = value(goal_type, row.amount);
        return if row.held_for_review {
            Outcome::Pending(v)
        } else {
            Outcome::Counted(v)
        };
    }

    // No category yet: it may still land in scope once resolved, so it is
    // pending when the scope has a category condition (tags-only scopes
    // simply do not match it). Only the spending side of a limit is held
    // this way, so unresolved income does not read as a negative pending.
    if row.category_slug.is_none()
        && scope.matches_if_category_resolved(&row.tags)
        && !(goal_type == GoalType::SpendingLimit && row.amount >= Decimal::ZERO)
    {
        return Outcome::Pending(value(goal_type, row.amount));
    }

    Outcome::Ignored
}

/// `(total, pending)` over rows.
pub fn tally(goal_type: GoalType, scope: &Scope, rows: &[ContributionRow]) -> (Decimal, Decimal) {
    rows.iter()
        .fold((Decimal::ZERO, Decimal::ZERO), |(t, p), row| {
            match classify(goal_type, scope, row) {
                Outcome::Counted(v) => (t + v, p),
                Outcome::Pending(v) => (t, p + v),
                Outcome::Ignored => (t, p),
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kafka::goals::Combine;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn scope(cats: &[&str], tags: &[&str]) -> Scope {
        Scope {
            category_slugs: cats.iter().map(|s| s.to_string()).collect(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            combine: Combine::All,
            tag_combine: Combine::All,
        }
    }

    fn row(amount: &str, slug: Option<&str>, kind: Option<&str>) -> ContributionRow {
        ContributionRow {
            transaction_id: Uuid::nil(),
            part_index: None,
            amount: d(amount),
            category_slug: slug.map(str::to_string),
            category_kind: kind.map(str::to_string),
            tags: vec![],
            booking_date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            held_for_review: false,
        }
    }

    #[test]
    fn spending_limit_sums_negative_amounts_as_positive_magnitude() {
        let s = scope(&["food"], &[]);
        let rows = [
            row("-10.50", Some("food"), Some("expense")),
            row("-4.25", Some("food.x"), Some("expense")),
        ];
        assert_eq!(tally(GoalType::SpendingLimit, &s, &rows), (d("14.75"), d("0")));
    }

    #[test]
    fn refund_reduces_a_spending_limit() {
        let s = scope(&["food"], &[]);
        let rows = [
            row("-30.00", Some("food"), Some("expense")),
            row("12.50", Some("food"), Some("expense")),
        ];
        assert_eq!(tally(GoalType::SpendingLimit, &s, &rows).0, d("17.50"));
    }

    #[test]
    fn saving_target_counts_only_saving_kind_as_magnitude() {
        let s = scope(&["savings", "food"], &[]);
        let rows = [
            row("-100.00", Some("savings.etf"), Some("saving")),
            row("40.00", Some("savings.etf"), Some("saving")),
            row("-55.00", Some("food"), Some("expense")),
        ];
        assert_eq!(tally(GoalType::SavingTarget, &s, &rows).0, d("140.00"));
    }

    #[test]
    fn transfer_kind_never_counts() {
        let s = scope(&["transfers"], &[]);
        let rows = [row("-50.00", Some("transfers"), Some("transfer"))];
        assert_eq!(tally(GoalType::SpendingLimit, &s, &rows), (d("0"), d("0")));
        assert_eq!(tally(GoalType::SavingTarget, &s, &rows), (d("0"), d("0")));
    }

    #[test]
    fn split_counts_by_parts_and_exactly_once() {
        let s = scope(&["food"], &[]);
        let id = Uuid::new_v4();
        let mut a = row("-30.00", Some("food"), Some("expense"));
        let mut b = row("-20.00", Some("leisure"), Some("expense"));
        a.transaction_id = id;
        a.part_index = Some(0);
        b.transaction_id = id;
        b.part_index = Some(1);
        // Only the in-scope part counts; the whole -50 is not counted again.
        assert_eq!(tally(GoalType::SpendingLimit, &s, &[a, b]).0, d("30.00"));
    }

    #[test]
    fn split_parts_inherit_transaction_tags() {
        let s = scope(&[], &["trip"]);
        let mut a = row("-30.00", Some("food"), Some("expense"));
        let mut b = row("-20.00", Some("leisure"), Some("expense"));
        a.tags = vec!["trip".into()];
        b.tags = vec!["trip".into()];
        assert_eq!(tally(GoalType::SpendingLimit, &s, &[a, b]).0, d("50.00"));
    }

    #[test]
    fn needs_review_lands_in_pending_not_total() {
        let s = scope(&["food"], &[]);
        let mut held = row("-9.00", Some("food"), Some("expense"));
        held.held_for_review = true;
        let ok = row("-1.00", Some("food"), Some("expense"));
        assert_eq!(
            tally(GoalType::SpendingLimit, &s, &[held, ok]),
            (d("1.00"), d("9.00"))
        );
    }

    #[test]
    fn uncategorised_row_is_pending_for_a_category_scope_only() {
        let uncat = row("-7.00", None, None);
        assert_eq!(
            tally(GoalType::SpendingLimit, &scope(&["food"], &[]), std::slice::from_ref(&uncat)),
            (d("0"), d("7.00"))
        );
        assert_eq!(
            tally(GoalType::SpendingLimit, &scope(&[], &["trip"]), std::slice::from_ref(&uncat)),
            (d("0"), d("0"))
        );
    }

    #[test]
    fn uncategorised_income_is_not_pending_for_a_limit() {
        let uncat = row("2000.00", None, None);
        assert_eq!(
            tally(GoalType::SpendingLimit, &scope(&["food"], &[]), &[uncat]),
            (d("0"), d("0"))
        );
    }

    #[test]
    fn exact_decimals_no_float_drift() {
        let s = scope(&["food"], &[]);
        let rows: Vec<_> = (0..10)
            .map(|_| row("-0.1", Some("food"), Some("expense")))
            .collect();
        assert_eq!(tally(GoalType::SpendingLimit, &s, &rows).0, d("1.0"));
    }
}
