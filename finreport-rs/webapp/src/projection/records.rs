//! Normalized, source-free shapes a [`SourceMapper`](super::mapper::SourceMapper)
//! produces (§2.4). These carry their own deterministic `id` (§2.3) and
//! timestamps taken from the record, so nothing downstream of a mapper adds
//! non-deterministic data — that is what makes a replay reproduce byte
//! identical tables.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::Value;
use uuid::Uuid;

/// `account.origin` / `account_balance.origin` / `transaction.origin` values
/// a mapper may produce. `Stub` is never returned by a mapper — it is
/// synthesized by the projector itself (§2.3) when a balance or transaction
/// arrives for an account it has not projected yet.
pub const ORIGIN_SOURCE: &str = "source";
pub const ORIGIN_LEGACY: &str = "legacy";
pub const ORIGIN_STUB: &str = "stub";

/// Normalized `account` row, source-free (§3).
#[derive(Debug, Clone, PartialEq)]
pub struct AccountRecord {
    pub id: Uuid,
    pub source: String,
    pub external_id: String,
    pub display_id: Option<String>,
    pub account_type: Option<String>,
    pub iban: Option<String>,
    pub bic: Option<String>,
    pub institute: Option<String>,
    pub label: Option<String>,
    pub currency: String,
    pub raw_payload: Option<Value>,
    /// `"source"` or `"legacy"` — never `"stub"` (§2.3).
    pub origin: String,
    pub first_seen_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Normalized `account_balance` row (§3).
#[derive(Debug, Clone, PartialEq)]
pub struct BalanceRecord {
    pub id: Uuid,
    /// Deterministic `account_uuid(source, source_account_id)` (§2.3) — the
    /// mapper computes this straight from the envelope, so it never needs a
    /// DB lookup to stay pure, and it is byte-identical to the id the owning
    /// `account` row will eventually have (or already has).
    pub account_id: Uuid,
    pub balance_date: NaiveDate,
    pub amount: Decimal,
    pub currency: String,
    pub raw_payload: Option<Value>,
    pub origin: String,
    pub observed_at: DateTime<Utc>,
}

/// Normalized `transaction` row (§3).
#[derive(Debug, Clone, PartialEq)]
pub struct TransactionRecord {
    pub id: Uuid,
    pub account_id: Uuid,
    pub source: String,
    pub external_id: String,
    pub booking_date: NaiveDate,
    pub valuta_date: Option<NaiveDate>,
    pub booking_status: String,
    pub amount: Decimal,
    pub currency: String,
    pub counterparty_name: Option<String>,
    pub counterparty_iban: Option<String>,
    pub description: Option<String>,
    pub transaction_type: Option<String>,
    pub raw_payload: Value,
    pub origin: String,
    pub imported_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Why a mapper could not turn one ingest record into a row. Every variant is
/// a **skip + log** outcome (§2.3) — poison records must not wedge the
/// pipeline, so this type deliberately has no "fatal" case; the caller always
/// treats it as "drop this one record and keep going".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapError {
    /// `source_account_id` was required for this entity kind but the header
    /// was absent (§2.2) — inventing an account would corrupt the read
    /// model, so this is a pure, deterministic failure, not a bug.
    MissingSourceAccountId,
    /// The payload was not valid JSON, or not the shape this mapper expects.
    InvalidPayload(String),
    /// A date field could not be parsed. `bookingDate` is required (§2.5):
    /// a transaction without a date is useless to every view in this
    /// iteration.
    UnparseableDate { field: &'static str, value: String },
    /// An amount field could not be parsed as a decimal.
    UnparseableAmount { field: &'static str, value: String },
}

impl std::fmt::Display for MapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MapError::MissingSourceAccountId => {
                write!(f, "missing source_account_id header")
            }
            MapError::InvalidPayload(msg) => write!(f, "invalid payload: {msg}"),
            MapError::UnparseableDate { field, value } => {
                write!(f, "unparseable date in {field}: {value:?}")
            }
            MapError::UnparseableAmount { field, value } => {
                write!(f, "unparseable amount in {field}: {value:?}")
            }
        }
    }
}

impl std::error::Error for MapError {}
