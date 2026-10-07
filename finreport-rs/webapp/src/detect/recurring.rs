//! Recurring-cost detection (iteration 3 §3.2): groups a user's
//! transactions by `(counterparty_key, sign(amount))`, then within each
//! group detects a stable monthly/quarterly/yearly cadence around a rolling
//! median amount.

use super::TxnFacts;
use chrono::NaiveDate;
use rust_decimal::prelude::{FromPrimitive, Signed};
use rust_decimal::Decimal;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Tunables for [`detect_recurring`] (iteration 3 §2.4).
#[derive(Debug, Clone, Copy)]
pub struct RecurringConfig {
    /// `APP_recurring_min_occurrences`.
    pub min_occurrences: u32,
    /// `APP_recurring_amount_tolerance`, relative to the series median.
    pub amount_tolerance: f32,
    /// `APP_recurring_window_months`: detection lookback.
    pub window_months: u32,
}

/// Cadence a [`RecurringSeries`] was detected at (§3.2), mirrors the
/// `recurring_cadence` column / `RecurringCadence` GraphQL enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    Monthly,
    Quarterly,
    Yearly,
}

impl Cadence {
    /// Single-period band, inclusive, in days (§3.2 rule 3).
    fn band(self) -> (i64, i64) {
        match self {
            Cadence::Monthly => (26, 35),
            Cadence::Quarterly => (83, 98),
            Cadence::Yearly => (350, 380),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Cadence::Monthly => "monthly",
            Cadence::Quarterly => "quarterly",
            Cadence::Yearly => "yearly",
        }
    }

    fn months(self) -> u32 {
        match self {
            Cadence::Monthly => 1,
            Cadence::Quarterly => 3,
            Cadence::Yearly => 12,
        }
    }

    fn next_expected(self, last: NaiveDate) -> NaiveDate {
        let months = i32::try_from(self.months()).unwrap_or(1);
        add_months(last, months)
    }
}

fn add_months(date: NaiveDate, months: i32) -> NaiveDate {
    use chrono::Datelike;
    let total = date.year() * 12 + (date.month() as i32 - 1) + months;
    let year = total.div_euclid(12);
    let month = total.rem_euclid(12) + 1;
    // Clamp day-of-month to the target month's length (e.g. Jan 31 + 1mo ->
    // Feb 28/29), rather than panicking on an invalid date.
    let mut day = date.day();
    loop {
        if let Some(d) = NaiveDate::from_ymd_opt(year, month as u32, day) {
            return d;
        }
        day -= 1;
    }
}

/// One detected recurring series (§3.2): `series_id =
/// UUIDv5(FINREPORT_NS, "recurring\0"+counterparty_key+"\0"+sign+"\0"+cadence)`,
/// flags every member transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurringSeries {
    pub series_id: Uuid,
    pub cadence: Cadence,
    /// Signed, `NUMERIC(20,4)`.
    pub median_amount: Decimal,
    pub occurrences: Vec<Uuid>,
    pub first_date: NaiveDate,
    pub last_date: NaiveDate,
    pub next_expected_date: NaiveDate,
}

/// `abs(amount)` median of a sorted-by-amount slice. `n >= 1` is guaranteed
/// by every caller.
fn median_abs(amounts: &[Decimal]) -> Decimal {
    let mut sorted: Vec<Decimal> = amounts.to_vec();
    sorted.sort();
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / Decimal::from(2)
    }
}

/// One occurrence inside a `(counterparty_key, sign)` group, carrying enough
/// to re-check the amount band and walk cadence gaps.
#[derive(Clone)]
struct Occurrence {
    id: Uuid,
    date: NaiveDate,
    amount: Decimal,
}

/// Tries every cadence monthly -> quarterly -> yearly against the date gaps
/// of `occurrences` (already sorted by date); first match wins (§3.2 rule
/// 3): every gap must fall in the cadence's single-period band, with at
/// most one gap allowed to fall in the doubled band instead (one skipped
/// period).
fn fits_cadence(occurrences: &[Occurrence], cadence: Cadence) -> bool {
    let (lo, hi) = cadence.band();
    let mut skipped_used = false;
    for pair in occurrences.windows(2) {
        let gap = (pair[1].date - pair[0].date).num_days();
        if gap >= lo && gap <= hi {
            continue;
        }
        if !skipped_used && gap >= lo * 2 && gap <= hi * 2 {
            skipped_used = true;
            continue;
        }
        return false;
    }
    true
}

/// Series id, deterministic across re-runs/replays/input order (§3.2 rule
/// 4): `UUIDv5(FINREPORT_NS, "recurring\0"+counterparty_key+"\0"+sign+"\0"+cadence)`.
fn series_id(counterparty_key: &str, sign: &str, cadence: Cadence) -> Uuid {
    let name = format!("recurring\0{counterparty_key}\0{sign}\0{}", cadence.label());
    Uuid::new_v5(&crate::kafka::envelope::FINREPORT_NS, name.as_bytes())
}

/// Detects a recurring series (if any) within one `(counterparty_key,
/// sign)` group's occurrences, already within the lookback window (§3.2).
fn detect_in_group(mut occurrences: Vec<Occurrence>, cfg: RecurringConfig) -> Option<RecurringSeries> {
    occurrences.sort_by_key(|o| o.date);

    // §3.2 rule 2: amount band re-checked after dropping out-of-band
    // occurrences, since dropping one can change the median.
    loop {
        if occurrences.len() < cfg.min_occurrences as usize {
            return None;
        }
        let amounts: Vec<Decimal> = occurrences.iter().map(|o| o.amount.abs()).collect();
        let median = median_abs(&amounts);
        let tolerance_amount = {
            let rel = median * Decimal::from_f32(cfg.amount_tolerance).unwrap_or_default();
            rel.max(Decimal::ONE)
        };
        let before = occurrences.len();
        occurrences.retain(|o| (o.amount.abs() - median).abs() <= tolerance_amount);
        if occurrences.len() == before {
            break;
        }
    }

    if occurrences.len() < cfg.min_occurrences as usize {
        return None;
    }

    let cadence = [Cadence::Monthly, Cadence::Quarterly, Cadence::Yearly]
        .into_iter()
        .find(|&c| fits_cadence(&occurrences, c))?;

    let amounts: Vec<Decimal> = occurrences.iter().map(|o| o.amount.abs()).collect();
    let median_abs_amount = median_abs(&amounts);
    // Sign is uniform within a group by construction (grouped on
    // `sign(amount)`); re-apply it to report the signed median (§3.2).
    let sign = occurrences[0].amount.signum();
    let median_amount = median_abs_amount * sign;

    let first_date = occurrences.first().map(|o| o.date).unwrap();
    let last_date = occurrences.last().map(|o| o.date).unwrap();

    Some(RecurringSeries {
        series_id: Uuid::nil(), // filled in by the caller, which knows the group key.
        cadence,
        median_amount,
        occurrences: occurrences.iter().map(|o| o.id).collect(),
        first_date,
        last_date,
        next_expected_date: cadence.next_expected(last_date),
    })
}

/// Groups `txns` by `(counterparty_key, sign(amount))` and detects a
/// recurring cadence within each group (§3.2). A transaction with no
/// `counterparty_key` is never recurring.
pub fn detect_recurring(
    txns: &[TxnFacts],
    cfg: RecurringConfig,
    today: NaiveDate,
) -> Vec<RecurringSeries> {
    let window_start = add_months(today, -(i32::try_from(cfg.window_months).unwrap_or(18)));

    // `BTreeMap` keeps iteration order deterministic regardless of input
    // order (§3.2 "series_id stable across ... input order").
    let mut groups: BTreeMap<(String, bool), Vec<Occurrence>> = BTreeMap::new();
    for t in txns {
        let Some(key) = t.counterparty_key.as_deref() else {
            continue;
        };
        if t.amount == Decimal::ZERO || t.booking_date < window_start || t.booking_date > today {
            continue;
        }
        let sign_positive = t.amount.is_sign_positive();
        groups
            .entry((key.to_string(), sign_positive))
            .or_default()
            .push(Occurrence {
                id: t.id,
                date: t.booking_date,
                amount: t.amount,
            });
    }

    let mut series = Vec::new();
    for ((key, sign_positive), occurrences) in groups {
        let sign_str = if sign_positive { "+" } else { "-" };
        if let Some(mut s) = detect_in_group(occurrences, cfg) {
            s.series_id = series_id(&key, sign_str, s.cadence);
            series.push(s);
        }
    }
    series
}

/// Monthly-equivalent cost (§3.2): `median_amount x {monthly 1, quarterly
/// 1/3, yearly 1/12}`, exact decimal, half-up to 4 dp.
pub fn monthly_equivalent(median_amount: Decimal, cadence: Cadence) -> Decimal {
    let factor = match cadence {
        Cadence::Monthly => Decimal::ONE,
        Cadence::Quarterly => Decimal::ONE / Decimal::from(3),
        Cadence::Yearly => Decimal::ONE / Decimal::from(12),
    };
    (median_amount * factor).round_dp_with_strategy(4, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn amt(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn cfg() -> RecurringConfig {
        RecurringConfig {
            min_occurrences: 3,
            amount_tolerance: 0.10,
            window_months: 18,
        }
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 12, 1).unwrap()
    }

    fn txn(id: Uuid, date: NaiveDate, amount: Decimal, key: &str) -> TxnFacts {
        TxnFacts {
            id,
            account_id: Uuid::new_v4(),
            source: "comdirect".to_string(),
            external_id: id.to_string(),
            owner_user_ids: vec![],
            booking_date: date,
            amount,
            counterparty_iban: None,
            account_iban: None,
            counterparty_key: Some(key.to_string()),
        }
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn stub_returns_no_series_when_empty() {
        assert_eq!(detect_recurring(&[], cfg(), today()), Vec::new());
    }

    #[test]
    fn exactly_three_qualifies_two_does_not() {
        let txns2 = vec![
            txn(Uuid::new_v4(), d(2024, 9, 1), amt("-39.90"), "gym"),
            txn(Uuid::new_v4(), d(2024, 10, 1), amt("-39.90"), "gym"),
        ];
        assert!(detect_recurring(&txns2, cfg(), today()).is_empty());

        let txns3 = vec![
            txn(Uuid::new_v4(), d(2024, 8, 1), amt("-39.90"), "gym"),
            txn(Uuid::new_v4(), d(2024, 9, 1), amt("-39.90"), "gym"),
            txn(Uuid::new_v4(), d(2024, 10, 1), amt("-39.90"), "gym"),
        ];
        let series = detect_recurring(&txns3, cfg(), today());
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].occurrences.len(), 3);
        assert_eq!(series[0].cadence, Cadence::Monthly);
    }

    #[test]
    fn monthly_band_boundaries() {
        // 26-day gaps, at the inside edge of the monthly band.
        let txns = vec![
            txn(Uuid::new_v4(), d(2024, 6, 1), amt("-10.00"), "k"),
            txn(Uuid::new_v4(), d(2024, 6, 27), amt("-10.00"), "k"),
            txn(Uuid::new_v4(), d(2024, 7, 23), amt("-10.00"), "k"),
        ];
        assert_eq!(detect_recurring(&txns, cfg(), today())[0].cadence, Cadence::Monthly);

        // 25-day gap is outside both monthly (26-35) and its double, so no
        // series.
        let too_tight = vec![
            txn(Uuid::new_v4(), d(2024, 6, 1), amt("-10.00"), "k2"),
            txn(Uuid::new_v4(), d(2024, 6, 26), amt("-10.00"), "k2"),
            txn(Uuid::new_v4(), d(2024, 7, 21), amt("-10.00"), "k2"),
        ];
        assert!(detect_recurring(&too_tight, cfg(), today()).is_empty());
    }

    #[test]
    fn quarterly_band_boundaries() {
        let txns = vec![
            txn(Uuid::new_v4(), d(2024, 1, 1), amt("-187.43"), "ins"),
            txn(Uuid::new_v4(), d(2024, 4, 4), amt("-187.43"), "ins"), // 94 days
            txn(Uuid::new_v4(), d(2024, 7, 6), amt("-187.43"), "ins"), // 93 days
        ];
        assert_eq!(detect_recurring(&txns, cfg(), today())[0].cadence, Cadence::Quarterly);
    }

    #[test]
    fn yearly_band_boundaries() {
        let txns = vec![
            txn(Uuid::new_v4(), d(2022, 1, 1), amt("-500.00"), "ins2"),
            txn(Uuid::new_v4(), d(2023, 1, 5), amt("-500.00"), "ins2"), // 369 days
            txn(Uuid::new_v4(), d(2024, 1, 5), amt("-500.00"), "ins2"), // 365 days
        ];
        let wide_window = RecurringConfig { window_months: 36, ..cfg() };
        assert_eq!(detect_recurring(&txns, wide_window, today())[0].cadence, Cadence::Yearly);
    }

    #[test]
    fn one_skipped_period_tolerated_two_not() {
        // 4 occurrences monthly but one 60-day (doubled-band) gap.
        let one_skip = vec![
            txn(Uuid::new_v4(), d(2024, 6, 1), amt("-39.90"), "gym2"),
            txn(Uuid::new_v4(), d(2024, 7, 1), amt("-39.90"), "gym2"),
            txn(Uuid::new_v4(), d(2024, 8, 30), amt("-39.90"), "gym2"), // 60-day gap
            txn(Uuid::new_v4(), d(2024, 9, 29), amt("-39.90"), "gym2"),
        ];
        assert_eq!(detect_recurring(&one_skip, cfg(), today())[0].cadence, Cadence::Monthly);

        // Two skipped-period gaps: not a series.
        let two_skips = vec![
            txn(Uuid::new_v4(), d(2024, 1, 1), amt("-39.90"), "gym3"),
            txn(Uuid::new_v4(), d(2024, 3, 1), amt("-39.90"), "gym3"), // 60-day gap
            txn(Uuid::new_v4(), d(2024, 5, 1), amt("-39.90"), "gym3"), // 61-day gap
            txn(Uuid::new_v4(), d(2024, 7, 1), amt("-39.90"), "gym3"),
        ];
        assert!(detect_recurring(&two_skips, cfg(), today()).is_empty());
    }

    #[test]
    fn out_of_band_amount_drops_occurrence_and_can_drop_series_below_three() {
        let txns = vec![
            txn(Uuid::new_v4(), d(2024, 8, 1), amt("-40.00"), "gym4"),
            txn(Uuid::new_v4(), d(2024, 9, 1), amt("-40.00"), "gym4"),
            txn(Uuid::new_v4(), d(2024, 10, 1), amt("-500.00"), "gym4"), // way out of band
        ];
        assert!(detect_recurring(&txns, cfg(), today()).is_empty());
    }

    #[test]
    fn drift_inside_band_keeps_one_series() {
        let txns = vec![
            txn(Uuid::new_v4(), d(2024, 7, 1), amt("-65.00"), "eon"),
            txn(Uuid::new_v4(), d(2024, 8, 1), amt("-65.00"), "eon"),
            txn(Uuid::new_v4(), d(2024, 9, 1), amt("-68.00"), "eon"),
            txn(Uuid::new_v4(), d(2024, 10, 1), amt("-70.00"), "eon"),
        ];
        let series = detect_recurring(&txns, cfg(), today());
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].occurrences.len(), 4);
    }

    #[test]
    fn series_id_stable_across_reruns_and_input_order() {
        let txns = vec![
            txn(Uuid::new_v4(), d(2024, 8, 1), amt("-39.90"), "gym5"),
            txn(Uuid::new_v4(), d(2024, 9, 1), amt("-39.90"), "gym5"),
            txn(Uuid::new_v4(), d(2024, 10, 1), amt("-39.90"), "gym5"),
        ];
        let mut reordered = txns.clone();
        reordered.reverse();

        let s1 = detect_recurring(&txns, cfg(), today());
        let s2 = detect_recurring(&reordered, cfg(), today());
        assert_eq!(s1[0].series_id, s2[0].series_id);
    }

    #[test]
    fn monthly_equivalent_exact_at_4dp() {
        let yearly = monthly_equivalent(amt("-1200.00"), Cadence::Yearly);
        assert_eq!(yearly, amt("-100.0000"));

        let quarterly = monthly_equivalent(amt("-100.00"), Cadence::Quarterly);
        assert_eq!(quarterly, amt("-33.3333"));
    }

    #[test]
    fn no_counterparty_key_never_recurring() {
        let mut txns = vec![
            txn(Uuid::new_v4(), d(2024, 8, 1), amt("-39.90"), "gym6"),
            txn(Uuid::new_v4(), d(2024, 9, 1), amt("-39.90"), "gym6"),
            txn(Uuid::new_v4(), d(2024, 10, 1), amt("-39.90"), "gym6"),
        ];
        for t in &mut txns {
            t.counterparty_key = None;
        }
        assert!(detect_recurring(&txns, cfg(), today()).is_empty());
    }
}
