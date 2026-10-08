//! Goal evaluation (iteration 4 §2.3, §3): progress is derived at read time
//! from the current labels, splits, tags and transfer flags, never stored.
//!
//! Only [`rows::fetch_rows`] touches SQL; the scope predicate
//! ([`scope`]), the counting rules ([`counting`]) and the period arithmetic
//! ([`periods`]) are pure functions over rows and dates.

pub mod counting;
pub mod periods;
pub mod rows;
pub mod scope;

use chrono::{NaiveDate, Utc};
use entity::entities::goal;
use rust_decimal::Decimal;
use sea_orm::{DatabaseConnection, DbErr};
use uuid::Uuid;

use counting::{classify, tally, ContributionRow, Outcome};
use periods::{plan_periods, Period, PeriodSpec};
use scope::Scope;

use crate::kafka::goals::GoalType;

/// The window a progress request covers.
pub struct ProgressWindow {
    pub start: chrono::NaiveDate,
    pub end: chrono::NaiveDate,
}

pub struct BucketProgress {
    pub start: chrono::NaiveDate,
    pub end: chrono::NaiveDate,
    /// Display label for the period, e.g. "Feb 2026", "Q1 2026", "2026".
    pub label: String,
    /// Positive magnitude of what counted in this bucket.
    pub total: rust_decimal::Decimal,
    /// Positive magnitude held for review, never included in `total`.
    pub pending: rust_decimal::Decimal,
    /// True for the bucket containing today.
    pub in_progress: bool,
}

pub struct GoalProgressData {
    pub buckets: Vec<BucketProgress>,
    /// Sum of every bucket's `total`.
    pub total: rust_decimal::Decimal,
    pub pending: rust_decimal::Decimal,
    /// Mean of `total` over completed buckets only; zero when there are none.
    pub average_per_period: rust_decimal::Decimal,
}

fn parse_goal_type(s: &str) -> GoalType {
    if s == "saving_target" {
        GoalType::SavingTarget
    } else {
        GoalType::SpendingLimit
    }
}

fn today() -> NaiveDate {
    Utc::now().date_naive()
}

/// Smallest and largest date covered by `periods`, if any.
fn span(periods: &[Period]) -> Option<(NaiveDate, NaiveDate)> {
    Some((periods.first()?.start, periods.last()?.end))
}

/// Pure core of [`evaluate_goal`]: tallies `rows` into `periods`.
/// A bucket is completed when it ended before `today`; `in_progress` buckets
/// (and future ones) are excluded from the average.
pub fn assemble(
    goal_type: GoalType,
    scope: &Scope,
    spec: PeriodSpec,
    periods: &[Period],
    rows: &[ContributionRow],
    today: NaiveDate,
) -> GoalProgressData {
    let mut buckets = Vec::with_capacity(periods.len());
    let mut completed_total = Decimal::ZERO;
    let mut completed_count = 0u32;

    for p in periods {
        let in_bucket: Vec<ContributionRow> = rows
            .iter()
            .filter(|r| r.booking_date >= p.start && r.booking_date <= p.end)
            .cloned()
            .collect();
        let (total, pending) = tally(goal_type, scope, &in_bucket);
        let in_progress = periods::is_in_progress(p, spec, today);
        if p.end < today && !in_progress {
            completed_total += total;
            completed_count += 1;
        }
        buckets.push(BucketProgress {
            start: p.start,
            end: p.end,
            label: p.label.clone(),
            total,
            pending,
            in_progress,
        });
    }

    let total = buckets.iter().map(|b| b.total).sum();
    let pending = buckets.iter().map(|b| b.pending).sum();
    let average_per_period = if completed_count == 0 {
        Decimal::ZERO
    } else {
        completed_total / Decimal::from(completed_count)
    };

    GoalProgressData {
        buckets,
        total,
        pending,
        average_per_period,
    }
}

/// Evaluates one goal. `window` None means the goal's default window:
/// the goal's own period for a fixed goal, and the last 12 periods ending
/// with the current one for a recurring goal.
pub async fn evaluate_goal(
    db: &DatabaseConnection,
    goal: &goal::Model,
    scoped_account_ids: &[Uuid],
    window: Option<ProgressWindow>,
) -> Result<GoalProgressData, DbErr> {
    let today = today();
    let goal_type = parse_goal_type(&goal.goal_type);
    let scope = Scope::from(goal);
    let Some(spec) = PeriodSpec::from_goal(goal) else {
        return Ok(assemble(goal_type, &scope, PeriodSpec::Recurring(crate::kafka::goals::Cadence::Monthly), &[], &[], today));
    };
    let window = window.map(|w| (w.start, w.end));
    let periods = plan_periods(spec, window, today);

    let rows = match span(&periods) {
        Some((from, to)) => rows::fetch_rows(db, scoped_account_ids, from, to).await?,
        None => Vec::new(),
    };
    Ok(assemble(goal_type, &scope, spec, &periods, &rows, today))
}

/// Ids of the transactions contributing to this goal within the inclusive
/// range, newest booking date first. Used for the drill-down list.
///
/// A transaction contributes when any of its rows is counted or held as
/// pending; a split with several in-scope parts is listed once.
pub async fn goal_transaction_ids(
    db: &DatabaseConnection,
    goal: &goal::Model,
    scoped_account_ids: &[Uuid],
    start: chrono::NaiveDate,
    end: chrono::NaiveDate,
) -> Result<Vec<Uuid>, DbErr> {
    let goal_type = parse_goal_type(&goal.goal_type);
    let scope = Scope::from(goal);
    let rows = rows::fetch_rows(db, scoped_account_ids, start, end).await?;
    Ok(contributing_ids(goal_type, &scope, rows))
}

/// Pure core of [`goal_transaction_ids`].
pub fn contributing_ids(
    goal_type: GoalType,
    scope: &Scope,
    mut rows: Vec<ContributionRow>,
) -> Vec<Uuid> {
    rows.retain(|r| classify(goal_type, scope, r) != Outcome::Ignored);
    rows.sort_by(|a, b| {
        b.booking_date
            .cmp(&a.booking_date)
            .then(a.transaction_id.cmp(&b.transaction_id))
    });
    let mut seen = std::collections::HashSet::new();
    rows.into_iter()
        .filter(|r| seen.insert(r.transaction_id))
        .map(|r| r.transaction_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kafka::goals::{Cadence, Combine};
    use std::str::FromStr;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }
    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }
    fn food_scope() -> Scope {
        Scope {
            category_slugs: vec!["food".into()],
            tags: vec![],
            combine: Combine::All,
            tag_combine: Combine::All,
        }
    }
    fn row(date: NaiveDate, amount: &str) -> ContributionRow {
        ContributionRow {
            transaction_id: Uuid::new_v4(),
            part_index: None,
            amount: dec(amount),
            category_slug: Some("food".into()),
            category_kind: Some("expense".into()),
            tags: vec![],
            booking_date: date,
            held_for_review: false,
        }
    }

    fn run(rows: &[ContributionRow], today: NaiveDate) -> GoalProgressData {
        let spec = PeriodSpec::Recurring(Cadence::Monthly);
        let periods = plan_periods(spec, Some((d(2026, 8, 1), d(2026, 10, 31))), today);
        assemble(GoalType::SpendingLimit, &food_scope(), spec, &periods, rows, today)
    }

    #[test]
    fn rows_land_in_their_calendar_month_and_totals_sum() {
        let rows = [
            row(d(2026, 8, 31), "-10"),
            row(d(2026, 9, 1), "-20.5"),
            row(d(2026, 9, 30), "-4.5"),
            row(d(2026, 10, 2), "-1"),
        ];
        let p = run(&rows, d(2026, 10, 8));
        let totals: Vec<_> = p.buckets.iter().map(|b| b.total).collect();
        assert_eq!(totals, vec![dec("10"), dec("25.0"), dec("1")]);
        assert_eq!(p.total, dec("36.0"));
    }

    #[test]
    fn current_bucket_is_in_progress_and_excluded_from_the_average() {
        let rows = [row(d(2026, 8, 5), "-10"), row(d(2026, 9, 5), "-20"), row(d(2026, 10, 5), "-1000")];
        let p = run(&rows, d(2026, 10, 8));
        let flags: Vec<_> = p.buckets.iter().map(|b| b.in_progress).collect();
        assert_eq!(flags, vec![false, false, true]);
        assert_eq!(p.average_per_period, dec("15"));
    }

    #[test]
    fn average_is_zero_with_no_completed_bucket() {
        let rows = [row(d(2026, 10, 5), "-10")];
        let spec = PeriodSpec::Recurring(Cadence::Monthly);
        let periods = plan_periods(spec, Some((d(2026, 10, 1), d(2026, 10, 31))), d(2026, 10, 8));
        let p = assemble(GoalType::SpendingLimit, &food_scope(), spec, &periods, &rows, d(2026, 10, 8));
        assert_eq!(p.average_per_period, Decimal::ZERO);
    }

    #[test]
    fn future_buckets_are_neither_in_progress_nor_averaged() {
        let p = run(&[], d(2026, 8, 15));
        let flags: Vec<_> = p.buckets.iter().map(|b| b.in_progress).collect();
        assert_eq!(flags, vec![true, false, false]);
        assert_eq!(p.average_per_period, Decimal::ZERO);
    }

    #[test]
    fn pending_is_summed_apart_from_total() {
        let mut held = row(d(2026, 9, 5), "-9");
        held.held_for_review = true;
        let p = run(&[held, row(d(2026, 9, 6), "-1")], d(2026, 10, 8));
        assert_eq!(p.total, dec("1"));
        assert_eq!(p.pending, dec("9"));
        assert_eq!(p.buckets[1].pending, dec("9"));
    }

    #[test]
    fn fixed_goal_is_one_bucket_over_its_range() {
        let spec = PeriodSpec::Fixed { start: d(2026, 1, 1), end: Some(d(2026, 6, 30)) };
        let today = d(2026, 10, 8);
        let periods = plan_periods(spec, None, today);
        let rows = [row(d(2026, 1, 1), "-5"), row(d(2026, 6, 30), "-5"), row(d(2026, 7, 1), "-99")];
        let p = assemble(GoalType::SpendingLimit, &food_scope(), spec, &periods, &rows, today);
        assert_eq!(p.buckets.len(), 1);
        assert_eq!(p.buckets[0].total, dec("10"));
        assert!(!p.buckets[0].in_progress);
        assert_eq!(p.average_per_period, dec("10"));
    }

    #[test]
    fn open_ended_fixed_goal_runs_to_today_and_is_in_progress() {
        let spec = PeriodSpec::Fixed { start: d(2026, 1, 1), end: None };
        let today = d(2026, 10, 8);
        let periods = plan_periods(spec, None, today);
        let p = assemble(GoalType::SpendingLimit, &food_scope(), spec, &periods, &[], today);
        assert_eq!(p.buckets[0].end, today);
        assert!(p.buckets[0].in_progress);
    }

    #[test]
    fn contributing_ids_list_a_split_once_newest_first() {
        let split = Uuid::new_v4();
        let mut a = row(d(2026, 9, 1), "-5");
        a.transaction_id = split;
        a.part_index = Some(0);
        let mut b = row(d(2026, 9, 1), "-6");
        b.transaction_id = split;
        b.part_index = Some(1);
        let newer = row(d(2026, 9, 9), "-1");
        let mut ignored = row(d(2026, 9, 10), "-1");
        ignored.category_slug = Some("rent".into());
        let ids = contributing_ids(GoalType::SpendingLimit, &food_scope(), vec![a, b, newer.clone(), ignored]);
        assert_eq!(ids, vec![newer.transaction_id, split]);
    }
}
