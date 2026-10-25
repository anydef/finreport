//! The `finreport.c24-account` and `finreport.c24-transaction` topics: rows of
//! a user-uploaded C24 CSV export, on topics of their own.
//!
//! # These two types are a wire contract
//!
//! [`C24AccountRecord`] and [`C24TransactionRecord`] are stored on compacted,
//! `prevent_destroy` topics, and their serde shape **is** the stored payload.
//! Renaming a field (or changing its type) silently breaks every record
//! already stored, so any such change needs a [`CURRENT_SCHEMA_VERSION`] bump
//! and a migration path, not just a passing test suite.
//!
//! That is exactly why `c24_csv::C24Row` is *not* the payload: the parser
//! crate stays free to be refactored, and [`C24TransactionRecord::from_row`]
//! is the one place that translates between the two. (`amazon_orders::Order`
//! being the payload of `finreport.purchase-order` is the trap this avoids.)
//!
//! # Why separate topics
//!
//! `finreport.transaction` promises the raw Comdirect JSON byte for byte, and
//! the projector deserialises its payloads as Comdirect JSON; a C24 row on it
//! would break that. There are two topics, not one, because
//! `transaction.account_id` is a real foreign key and an account row has to
//! exist (or be stubbed) for its transactions.
//!
//! There is deliberately **no** `c24-account-balance` topic: the C24 export
//! has no balance column at all, so there is nothing to publish.
//!
//! # Keys and headers
//!
//! Each topic is keyed by the entity's `external_id` (the account number /
//! the row's derived id). Headers use `source = "c24"`,
//! `SourceContext::Unattributed` (no `comdirect_*` headers), and the
//! transaction topic carries the account number as `source_account_id`.
//!
//! C24's own `Kategorie`/`Unterkategorie` ride along as evidence only; they
//! have no column and must never override a user override or a rule.

use c24_csv::C24Row;
use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Keyed by account number (`Kontonummer`). Compacted, 1 partition,
/// `prevent_destroy`.
pub const TOPIC_C24_ACCOUNT: &str = "finreport.c24-account";
/// Keyed by the row's derived `external_id`. Compacted, 1 partition,
/// `prevent_destroy`.
pub const TOPIC_C24_TRANSACTION: &str = "finreport.c24-transaction";

/// The `source` header / column value for C24 (lowercase, per §2.2).
pub const SOURCE_C24: &str = "c24";

/// Payload schema version for both record types. Bump on any change to their
/// serde shape.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// `booking_status` of every C24 row: the export holds booked rows only. The
/// same literal `ComdirectMapper` stores for a booked Comdirect transaction,
/// so the two sources agree.
pub const BOOKING_STATUS_BOOKED: &str = "BOOKED";

/// The currency of every C24 row. The export has no currency column.
const CURRENCY_EUR: &str = "EUR";

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Published on [`TOPIC_C24_ACCOUNT`]. Wire contract, see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct C24AccountRecord {
    pub schema_version: u32,
    /// `Kontonummer`.
    pub external_id: String,
    /// `Kontoname`, e.g. `C24 Smartkonto`.
    pub label: Option<String>,
    /// Always `None` for now: the export's `IBAN` column is the
    /// *counterparty's*, not the account's, so it must not be used here.
    pub iban: Option<String>,
}

impl C24AccountRecord {
    pub fn from_row(row: &C24Row) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            external_id: row.account_number.clone(),
            label: non_empty(&row.account_name),
            // NOT `row.iban`: that is the counterparty's IBAN.
            iban: None,
        }
    }

    pub fn topic_key(&self) -> &str {
        &self.external_id
    }
}

/// Published on [`TOPIC_C24_TRANSACTION`]. Wire contract, see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct C24TransactionRecord {
    pub schema_version: u32,
    /// The row's derived `<content_key>-<occurrence>` id.
    pub external_id: String,
    pub booking_date: NaiveDate,
    /// Always `None`: the export has no value-date column.
    pub valuta_date: Option<NaiveDate>,
    pub booking_status: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub amount: Decimal,
    pub currency: String,
    /// `Zahlungsempfänger`.
    pub counterparty_name: Option<String>,
    /// The counterparty's IBAN (`IBAN` column).
    pub counterparty_iban: Option<String>,
    /// `Transaktionstyp`, verbatim.
    pub transaction_type: String,
    /// `Verwendungszweck` when present, else `Beschreibung`.
    pub description: Option<String>,
    /// `Karteneinsatz`, kept as evidence; no column.
    pub card_used_at: Option<NaiveDateTime>,
    /// C24's own `Kategorie`: evidence only, never a decision.
    pub c24_category: Option<String>,
    /// C24's own `Unterkategorie`: evidence only.
    pub c24_subcategory: Option<String>,
}

impl C24TransactionRecord {
    pub fn from_row(row: &C24Row) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            external_id: row.external_id.clone(),
            booking_date: row.booking_date,
            valuta_date: None,
            booking_status: BOOKING_STATUS_BOOKED.to_string(),
            amount: row.amount,
            // The export has no currency column: every observed amount cell
            // carries `€` and the parser drops the symbol. If a non-EUR cell
            // ever appears, carrying or rejecting it is the *parser's* job;
            // `currency` stays an explicit field so only this line changes.
            currency: CURRENCY_EUR.to_string(),
            counterparty_name: non_empty(&row.counterparty),
            counterparty_iban: row.iban.as_deref().and_then(non_empty),
            transaction_type: row.transaction_type.clone(),
            description: row
                .purpose
                .as_deref()
                .and_then(non_empty)
                .or_else(|| row.description.as_deref().and_then(non_empty)),
            card_used_at: row.card_used_at,
            c24_category: row.category.as_ref().map(|c| c.category.clone()),
            c24_subcategory: row.category.as_ref().and_then(|c| c.subcategory.clone()),
        }
    }

    pub fn topic_key(&self) -> &str {
        &self.external_id
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use c24_csv::C24Category;

    /// A synthetic card payment: no IBAN, BIC or purpose.
    pub fn card_row() -> C24Row {
        C24Row {
            transaction_type: "Kartenzahlung".to_string(),
            booking_date: NaiveDate::from_ymd_opt(2026, 3, 4).unwrap(),
            card_used_at: NaiveDate::from_ymd_opt(2026, 3, 3)
                .unwrap()
                .and_hms_opt(18, 42, 0),
            amount: Decimal::new(-1250, 2),
            counterparty: "Beispiel Baeckerei".to_string(),
            iban: None,
            bic: None,
            purpose: None,
            description: None,
            account_number: "0000012345".to_string(),
            account_name: "Testkonto".to_string(),
            category: Some(C24Category {
                category: "Essen".to_string(),
                subcategory: Some("Baeckerei".to_string()),
            }),
            cash_withdrawal: None,
            content_key: "aa".repeat(32),
            occurrence: 1,
            external_id: format!("{}-1", "aa".repeat(32)),
        }
    }

    /// A synthetic transfer: IBAN and purpose present, no card time.
    pub fn transfer_row() -> C24Row {
        C24Row {
            transaction_type: "Echtzeit-Überweisung".to_string(),
            booking_date: NaiveDate::from_ymd_opt(2026, 3, 5).unwrap(),
            card_used_at: None,
            amount: Decimal::new(150000, 2),
            counterparty: "Max Mustermann".to_string(),
            iban: Some("DE00000000000000000000".to_string()),
            bic: Some("TESTDE00XXX".to_string()),
            purpose: Some("Miete Maerz".to_string()),
            description: Some("Beschreibung".to_string()),
            account_number: "0000012345".to_string(),
            account_name: "Testkonto".to_string(),
            category: None,
            cash_withdrawal: None,
            content_key: "bb".repeat(32),
            occurrence: 1,
            external_id: format!("{}-1", "bb".repeat(32)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn card_payment_maps_without_iban_or_purpose() {
        let row = card_row();
        let rec = C24TransactionRecord::from_row(&row);
        assert_eq!(rec.external_id, row.external_id);
        assert_eq!(rec.valuta_date, None);
        assert_eq!(rec.booking_status, "BOOKED");
        assert_eq!(rec.currency, "EUR");
        assert_eq!(rec.counterparty_name.as_deref(), Some("Beispiel Baeckerei"));
        assert_eq!(rec.counterparty_iban, None);
        assert_eq!(rec.description, None);
        assert_eq!(rec.transaction_type, "Kartenzahlung");
        assert_eq!(rec.c24_category.as_deref(), Some("Essen"));
        assert_eq!(rec.c24_subcategory.as_deref(), Some("Baeckerei"));
        assert_eq!(rec.topic_key(), row.external_id);
    }

    #[test]
    fn transfer_prefers_purpose_over_description() {
        let rec = C24TransactionRecord::from_row(&transfer_row());
        assert_eq!(rec.counterparty_iban.as_deref(), Some("DE00000000000000000000"));
        assert_eq!(rec.description.as_deref(), Some("Miete Maerz"));
        assert_eq!(rec.amount, Decimal::new(150000, 2));
    }

    #[test]
    fn description_is_the_fallback_when_there_is_no_purpose() {
        let mut row = transfer_row();
        row.purpose = None;
        assert_eq!(
            C24TransactionRecord::from_row(&row).description.as_deref(),
            Some("Beschreibung")
        );
    }

    #[test]
    fn account_record_never_takes_the_counterparty_iban() {
        let acct = C24AccountRecord::from_row(&transfer_row());
        assert_eq!(acct.external_id, "0000012345");
        assert_eq!(acct.label.as_deref(), Some("Testkonto"));
        assert_eq!(acct.iban, None);
        assert_eq!(acct.topic_key(), "0000012345");
    }

    #[test]
    fn records_round_trip_through_json() {
        let tx = C24TransactionRecord::from_row(&transfer_row());
        let back: C24TransactionRecord =
            serde_json::from_slice(&serde_json::to_vec(&tx).unwrap()).unwrap();
        assert_eq!(tx, back);
        let acct = C24AccountRecord::from_row(&card_row());
        let back: C24AccountRecord =
            serde_json::from_slice(&serde_json::to_vec(&acct).unwrap()).unwrap();
        assert_eq!(acct, back);
    }

    /// Freezes the stored shape. If this fails, you are changing the wire
    /// contract: bump `CURRENT_SCHEMA_VERSION` and add a migration path.
    #[test]
    fn transaction_wire_shape_is_frozen() {
        let json = serde_json::to_value(C24TransactionRecord::from_row(&card_row())).unwrap();
        let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "amount",
                "booking_date",
                "booking_status",
                "c24_category",
                "c24_subcategory",
                "card_used_at",
                "counterparty_iban",
                "counterparty_name",
                "currency",
                "description",
                "external_id",
                "schema_version",
                "transaction_type",
                "valuta_date"
            ]
        );
        assert_eq!(json["amount"], "-12.50");
    }

    #[test]
    fn account_wire_shape_is_frozen() {
        let json = serde_json::to_value(C24AccountRecord::from_row(&card_row())).unwrap();
        let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["external_id", "iban", "label", "schema_version"]);
    }
}
