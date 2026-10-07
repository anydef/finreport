//! Iteration 3 §2.2: the detector's `finreport.transaction-insight` topic —
//! the auto layer for internal transfers and recurring costs. Frozen by WP0;
//! produced by WP-A's `detect::processor`, consumed by WP-A's
//! `projection::insights` and merged with the user override layer
//! (`transaction_user_label.recurring`, §2.1) at read time by WP-B.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Detector output: one row per transaction's current auto-detected flags.
/// Keyed `<source>:<external_id>`, compacted, partitions 1, RF 1,
/// `prevent_destroy`. A tombstone deletes the row (§2.2).
pub const TOPIC_TRANSACTION_INSIGHT: &str = "finreport.transaction-insight";

/// `origin` header value for every record on [`TOPIC_TRANSACTION_INSIGHT`]
/// (§2.2) — distinct from `ORIGIN_USER`/`ORIGIN_LABELER`
/// (`crate::kafka::labeling`): this topic is never human-edited.
pub const ORIGIN_DETECTOR: &str = "detector";

/// Current payload schema version for [`InsightRecord`] (§2.2).
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Which bucket a transfer pair matched on (§3.1). Mirrors
/// `transaction_insight.transfer_match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferMatch {
    Iban,
    AmountDate,
}

/// A series' cadence (§3.2). Mirrors `transaction_insight.recurring_cadence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurringCadence {
    Monthly,
    Quarterly,
    Yearly,
}

/// Published on [`TOPIC_TRANSACTION_INSIGHT`] (§2.2, §3). Mirrors the
/// `transaction_insight` columns (§2.3) plus the transaction identity and
/// `schema_version`. Series attributes (`recurring_cadence`,
/// `recurring_median_amount`) are denormalized onto every member record
/// rather than living on a second, series-level topic (§2.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsightRecord {
    pub schema_version: u32,
    pub source: String,
    pub external_id: String,
    pub is_transfer: bool,
    /// `<source>:<external_id>` of the other leg; `None` while it is not
    /// projected yet (§2.2).
    pub transfer_counterpart: Option<String>,
    pub transfer_match: Option<TransferMatch>,
    pub is_recurring: bool,
    pub recurring_series_id: Option<uuid::Uuid>,
    pub recurring_cadence: Option<RecurringCadence>,
    /// Signed, `NUMERIC(20,4)` (§3.2).
    pub recurring_median_amount: Option<Decimal>,
    pub detected_at: DateTime<Utc>,
    /// RFC 3339; last-writer-wins, same shape as every other human/detector
    /// record (§2.1/§2.2).
    pub revision: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insight_record_round_trips_through_json() {
        let record = InsightRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            source: "comdirect".to_string(),
            external_id: "ACC1-TRANSFER-OUT-01".to_string(),
            is_transfer: true,
            transfer_counterpart: Some("comdirect:ACC2-TRANSFER-IN-01".to_string()),
            transfer_match: Some(TransferMatch::Iban),
            is_recurring: false,
            recurring_series_id: None,
            recurring_cadence: None,
            recurring_median_amount: None,
            detected_at: Utc::now(),
            revision: Utc::now(),
        };

        let json = serde_json::to_string(&record).unwrap();
        let round_tripped: InsightRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, round_tripped);
    }

    #[test]
    fn insight_record_round_trips_a_recurring_series_member() {
        let record = InsightRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            source: "comdirect".to_string(),
            external_id: "ACC1-RECURRING-MONTHLY-01".to_string(),
            is_transfer: false,
            transfer_counterpart: None,
            transfer_match: None,
            is_recurring: true,
            recurring_series_id: Some(uuid::Uuid::new_v4()),
            recurring_cadence: Some(RecurringCadence::Monthly),
            recurring_median_amount: Some(Decimal::new(-3990, 2)),
            detected_at: Utc::now(),
            revision: Utc::now(),
        };

        let json = serde_json::to_string(&record).unwrap();
        let round_tripped: InsightRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, round_tripped);
    }
}
