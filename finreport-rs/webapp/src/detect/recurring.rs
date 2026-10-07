//! Recurring-cost detection (iteration 3 §3.2). **WP-A owns the real
//! body** of [`detect_recurring`]; WP0 only freezes the signature.

use super::TxnFacts;
use chrono::NaiveDate;
use rust_decimal::Decimal;
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

/// Groups `txns` by `(counterparty_key, sign(amount))` and detects a
/// recurring cadence within each group (§3.2). **Stub**: WP0 freezes the
/// signature only — always returns `vec![]` until WP-A implements the
/// median/band/cadence walk.
pub fn detect_recurring(
    _txns: &[TxnFacts],
    _cfg: RecurringConfig,
    _today: NaiveDate,
) -> Vec<RecurringSeries> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_returns_no_series() {
        let cfg = RecurringConfig {
            min_occurrences: 3,
            amount_tolerance: 0.10,
            window_months: 18,
        };
        let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(detect_recurring(&[], cfg, today), Vec::new());
    }
}
