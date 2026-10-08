//! Period arithmetic (iteration 4 §3.4): pure. Calendar months come from
//! iteration 2's cashflow bucketing (`fill_dense_buckets`); quarters and
//! years are those months grouped, so there is exactly one implementation of
//! "where does a calendar period start and end".

use chrono::{Datelike, Months, NaiveDate};
use entity::entities::goal;

use crate::graphql::cashflow::summary::fill_dense_buckets;
use crate::graphql::types::Granularity;
use crate::kafka::goals::Cadence;

/// One bucket's date span and display label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Period {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub label: String,
}

/// A goal's period, parsed from the `goal` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodSpec {
    Recurring(Cadence),
    /// `end: None` is open-ended (runs to today).
    Fixed {
        start: NaiveDate,
        end: Option<NaiveDate>,
    },
}

impl PeriodSpec {
    /// `None` for a fixed goal missing its start date (cannot happen for
    /// validated records).
    pub fn from_goal(g: &goal::Model) -> Option<Self> {
        if g.period_kind == "fixed" {
            g.period_start.map(|start| PeriodSpec::Fixed {
                start,
                end: g.period_end,
            })
        } else {
            Some(PeriodSpec::Recurring(match g.period_cadence.as_deref() {
                Some("quarterly") => Cadence::Quarterly,
                Some("yearly") => Cadence::Yearly,
                _ => Cadence::Monthly,
            }))
        }
    }
}

fn months_per_period(cadence: Cadence) -> u32 {
    match cadence {
        Cadence::Monthly => 1,
        Cadence::Quarterly => 3,
        Cadence::Yearly => 12,
    }
}

/// First day of the calendar period containing `date`.
pub fn period_start(date: NaiveDate, cadence: Cadence) -> NaiveDate {
    let m = months_per_period(cadence);
    let first_month = ((date.month() - 1) / m) * m + 1;
    NaiveDate::from_ymd_opt(date.year(), first_month, 1).unwrap()
}

/// Last day of the calendar period containing `date`.
pub fn period_end(date: NaiveDate, cadence: Cadence) -> NaiveDate {
    let next = period_start(date, cadence) + Months::new(months_per_period(cadence));
    next.pred_opt().unwrap()
}

fn label(start: NaiveDate, cadence: Cadence) -> String {
    match cadence {
        Cadence::Monthly => start.format("%b %Y").to_string(),
        Cadence::Quarterly => format!("Q{} {}", (start.month() - 1) / 3 + 1, start.year()),
        Cadence::Yearly => start.year().to_string(),
    }
}

/// The default recurring window: the last 12 periods ending with the one
/// containing `today`.
pub fn default_recurring_window(cadence: Cadence, today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let current_start = period_start(today, cadence);
    let back = Months::new(months_per_period(cadence) * 11);
    (current_start - back, period_end(today, cadence))
}

/// Every calendar period of `cadence` intersecting `[from, to]`, in order.
/// Each is the full calendar period, not clipped to the window.
pub fn recurring_periods(cadence: Cadence, from: NaiveDate, to: NaiveDate) -> Vec<Period> {
    if from > to {
        return Vec::new();
    }
    let aligned_start = period_start(from, cadence);
    let aligned_end = period_end(to, cadence);
    let months = fill_dense_buckets(&[], aligned_start, aligned_end, Granularity::Month);

    let mut periods: Vec<Period> = Vec::new();
    for m in months {
        let start = period_start(m.start, cadence);
        match periods.last_mut() {
            Some(p) if p.start == start => p.end = m.end,
            _ => periods.push(Period {
                start,
                end: m.end,
                label: label(start, cadence),
            }),
        }
    }
    periods
}

/// The one bucket of a fixed goal, clipped to `window` when one is given.
/// Empty when the window misses the goal's range.
pub fn fixed_periods(
    start: NaiveDate,
    end: Option<NaiveDate>,
    window: Option<(NaiveDate, NaiveDate)>,
    today: NaiveDate,
) -> Vec<Period> {
    let goal_end = end.unwrap_or(today);
    let (from, to) = match window {
        Some((ws, we)) => (start.max(ws), goal_end.min(we)),
        None => (start, goal_end),
    };
    if from > to {
        return Vec::new();
    }
    vec![Period {
        start: from,
        end: to,
        label: format!("{from} – {to}"),
    }]
}

/// The periods a request covers. `window` None means the goal's default.
pub fn plan_periods(
    spec: PeriodSpec,
    window: Option<(NaiveDate, NaiveDate)>,
    today: NaiveDate,
) -> Vec<Period> {
    match spec {
        PeriodSpec::Recurring(cadence) => {
            let (from, to) = window.unwrap_or_else(|| default_recurring_window(cadence, today));
            recurring_periods(cadence, from, to)
        }
        PeriodSpec::Fixed { start, end } => fixed_periods(start, end, window, today),
    }
}

/// Whether the bucket contains today, judged against the goal's real range
/// (a fixed bucket clipped by a window still belongs to a goal that is
/// running).
pub fn is_in_progress(period: &Period, spec: PeriodSpec, today: NaiveDate) -> bool {
    match spec {
        PeriodSpec::Recurring(_) => period.start <= today && today <= period.end,
        PeriodSpec::Fixed { start, end } => {
            start <= today && today <= end.unwrap_or(today) && period.start <= today && today <= period.end
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn monthly_boundaries_including_leap_february() {
        let p = recurring_periods(Cadence::Monthly, d(2028, 1, 31), d(2028, 3, 1));
        let spans: Vec<_> = p.iter().map(|p| (p.start, p.end)).collect();
        assert_eq!(
            spans,
            vec![
                (d(2028, 1, 1), d(2028, 1, 31)),
                (d(2028, 2, 1), d(2028, 2, 29)),
                (d(2028, 3, 1), d(2028, 3, 31)),
            ]
        );
        assert_eq!(p[0].label, "Jan 2028");
    }

    #[test]
    fn quarterly_boundaries_and_labels() {
        let p = recurring_periods(Cadence::Quarterly, d(2026, 2, 10), d(2026, 8, 1));
        let spans: Vec<_> = p.iter().map(|p| (p.start, p.end, p.label.as_str())).collect();
        assert_eq!(
            spans,
            vec![
                (d(2026, 1, 1), d(2026, 3, 31), "Q1 2026"),
                (d(2026, 4, 1), d(2026, 6, 30), "Q2 2026"),
                (d(2026, 7, 1), d(2026, 9, 30), "Q3 2026"),
            ]
        );
    }

    #[test]
    fn yearly_boundaries_and_labels() {
        let p = recurring_periods(Cadence::Yearly, d(2025, 12, 31), d(2026, 1, 1));
        let spans: Vec<_> = p.iter().map(|p| (p.start, p.end, p.label.as_str())).collect();
        assert_eq!(
            spans,
            vec![
                (d(2025, 1, 1), d(2025, 12, 31), "2025"),
                (d(2026, 1, 1), d(2026, 12, 31), "2026"),
            ]
        );
    }

    #[test]
    fn default_window_is_twelve_periods_ending_with_the_current_one() {
        let today = d(2026, 10, 8);
        let (from, to) = default_recurring_window(Cadence::Monthly, today);
        assert_eq!((from, to), (d(2025, 11, 1), d(2026, 10, 31)));
        assert_eq!(recurring_periods(Cadence::Monthly, from, to).len(), 12);
        let (qf, qt) = default_recurring_window(Cadence::Quarterly, today);
        assert_eq!((qf, qt), (d(2024, 1, 1), d(2026, 12, 31)));
        assert_eq!(recurring_periods(Cadence::Quarterly, qf, qt).len(), 12);
        let (yf, yt) = default_recurring_window(Cadence::Yearly, today);
        assert_eq!((yf, yt), (d(2015, 1, 1), d(2026, 12, 31)));
        assert_eq!(recurring_periods(Cadence::Yearly, yf, yt).len(), 12);
    }

    #[test]
    fn open_ended_fixed_range_ends_today() {
        let today = d(2026, 10, 8);
        let p = fixed_periods(d(2026, 1, 1), None, None, today);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].start, p[0].end), (d(2026, 1, 1), today));
    }

    #[test]
    fn fixed_range_is_clipped_to_a_window_and_empty_when_missed() {
        let today = d(2026, 10, 8);
        let p = fixed_periods(
            d(2026, 1, 1),
            Some(d(2026, 6, 30)),
            Some((d(2026, 3, 1), d(2026, 12, 31))),
            today,
        );
        assert_eq!((p[0].start, p[0].end), (d(2026, 3, 1), d(2026, 6, 30)));
        assert!(fixed_periods(d(2026, 1, 1), Some(d(2026, 2, 1)), Some((d(2026, 5, 1), d(2026, 6, 1))), today).is_empty());
    }

    #[test]
    fn in_progress_marks_only_the_bucket_with_today() {
        let today = d(2026, 10, 8);
        let spec = PeriodSpec::Recurring(Cadence::Monthly);
        let p = recurring_periods(Cadence::Monthly, d(2026, 9, 1), d(2026, 11, 30));
        let flags: Vec<_> = p.iter().map(|p| is_in_progress(p, spec, today)).collect();
        assert_eq!(flags, vec![false, true, false]);
    }

    #[test]
    fn finished_fixed_goal_is_never_in_progress() {
        let today = d(2026, 10, 8);
        let spec = PeriodSpec::Fixed { start: d(2026, 1, 1), end: Some(d(2026, 6, 30)) };
        let p = fixed_periods(d(2026, 1, 1), Some(d(2026, 6, 30)), None, today);
        assert!(!is_in_progress(&p[0], spec, today));
    }
}
