//! `ComdirectMapper`: `(comdirect, source)` (§2.4, §2.5). Reuses
//! `comdirect-rs`'s parsed structs for `Account`/`Transaction` — they are
//! already public and shaped exactly like the bank's JSON — but never leaks
//! Comdirect vocabulary into the normalized records: everything that isn't
//! mapped to a normalized field lands in `raw_payload` instead.
//!
//! `comdirect_rs::comdirect::balance_model::Balance` is not reused for the
//! balance payload: its `unit` field is private (an implementation detail of
//! that crate's REST client, not something WP3 owns or can change), so this
//! module parses the `{value, unit}` shape with its own minimal struct.

use chrono::NaiveDate;
use comdirect_rs::comdirect::balance_model::Account as ComdirectAccount;
use comdirect_rs::comdirect::transaction::Transaction as ComdirectTransaction;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;
use std::str::FromStr;

use crate::kafka::envelope::{account_uuid, balance_uuid, transaction_uuid};
use crate::projection::mapper::{MapperInput, SourceMapper};
use crate::projection::records::{
    AccountRecord, BalanceRecord, MapError, TransactionRecord, ORIGIN_SOURCE,
};

pub struct ComdirectMapper;

/// The raw balance sub-object Comdirect returns alongside an account
/// (`{value, unit}`, §2.2/§2.5) — the live endpoint carries no date, so
/// `map_balance` below uses the record's own `imported_at` as the observed
/// date (see the comment there).
#[derive(Deserialize)]
struct ComdirectBalance {
    value: String,
    unit: Option<String>,
}

fn parse_json(payload: &[u8]) -> Result<Value, MapError> {
    serde_json::from_slice(payload).map_err(|e| MapError::InvalidPayload(e.to_string()))
}

fn parse_decimal(field: &'static str, value: &str) -> Result<Decimal, MapError> {
    Decimal::from_str(value).map_err(|_| MapError::UnparseableAmount {
        field,
        value: value.to_string(),
    })
}

fn parse_date(field: &'static str, value: &str) -> Result<NaiveDate, MapError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| MapError::UnparseableDate {
        field,
        value: value.to_string(),
    })
}

/// `creditor.holderName` -> `remitter.holderName` -> `deptor`, first
/// non-empty (§2.5).
fn pick_counterparty_name(tx: &ComdirectTransaction) -> Option<String> {
    let creditor_name = tx
        .creditor
        .as_ref()
        .map(|c| c.holder_name.as_str())
        .filter(|s| !s.is_empty());
    let remitter_name = tx
        .remitter
        .as_ref()
        .map(|r| r.holder_name.as_str())
        .filter(|s| !s.is_empty());
    let deptor_name = tx.deptor.as_deref().filter(|s| !s.is_empty());

    creditor_name
        .or(remitter_name)
        .or(deptor_name)
        .map(str::to_string)
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn require_source_account_id<'a>(input: &MapperInput<'a>) -> Result<&'a str, MapError> {
    input.event.source_account_id.ok_or(MapError::MissingSourceAccountId)
}

impl SourceMapper for ComdirectMapper {
    fn source(&self) -> &'static str {
        "comdirect"
    }

    fn map_account(&self, input: &MapperInput<'_>) -> Result<AccountRecord, MapError> {
        let account: ComdirectAccount = serde_json::from_slice(input.event.payload)
            .map_err(|e| MapError::InvalidPayload(e.to_string()))?;
        let raw_payload = parse_json(input.event.payload)?;

        let id = account_uuid(input.event.source, &account.account_id);

        Ok(AccountRecord {
            id,
            source: input.event.source.to_string(),
            external_id: account.account_id,
            display_id: non_empty(&account.display_id),
            account_type: non_empty(&account.account_type.text),
            iban: non_empty(&account.iban),
            bic: non_empty(&account.bic),
            // Comdirect's account payload names no institute separately
            // from `bic` (§2.5 does not map one); left for a later source
            // that has one.
            institute: None,
            label: input.comdirect_account_name.map(str::to_string),
            // No currency field on the Comdirect account payload (§3's
            // schema default applies).
            currency: "EUR".to_string(),
            raw_payload: Some(raw_payload),
            origin: ORIGIN_SOURCE.to_string(),
            first_seen_at: input.event.imported_at,
            updated_at: input.event.imported_at,
        })
    }

    fn map_balance(&self, input: &MapperInput<'_>) -> Result<BalanceRecord, MapError> {
        let source_account_id = require_source_account_id(input)?;
        let balance: ComdirectBalance = serde_json::from_slice(input.event.payload)
            .map_err(|e| MapError::InvalidPayload(e.to_string()))?;
        let raw_payload = parse_json(input.event.payload)?;

        let account_id = account_uuid(input.event.source, source_account_id);
        let amount = parse_decimal("value", &balance.value)?;
        // Comdirect's balances endpoint returns a live snapshot with no date
        // field at all (§2.5 describes a payload date that this endpoint
        // does not actually carry) — the only timestamp available for a
        // "source" balance is when the importer fetched it, so that is what
        // dates this observation. This is deliberately distinct from the
        // forbidden "replay-time now()": `imported_at` comes from the
        // envelope header, so it is fixed in the log and replay-stable.
        let balance_date = input.event.imported_at.date_naive();
        let id = balance_uuid(account_id, balance_date);

        Ok(BalanceRecord {
            id,
            account_id,
            balance_date,
            amount,
            currency: balance.unit.unwrap_or_else(|| "EUR".to_string()),
            raw_payload: Some(raw_payload),
            origin: ORIGIN_SOURCE.to_string(),
            observed_at: input.event.imported_at,
        })
    }

    fn map_transaction(&self, input: &MapperInput<'_>) -> Result<TransactionRecord, MapError> {
        let source_account_id = require_source_account_id(input)?;
        let tx: ComdirectTransaction = serde_json::from_slice(input.event.payload)
            .map_err(|e| MapError::InvalidPayload(e.to_string()))?;
        let raw_payload = parse_json(input.event.payload)?;

        let account_id = account_uuid(input.event.source, source_account_id);
        // `external_id` is Comdirect's own `reference` (§2.5) — the Kafka
        // message key, not the mapper's input, is allowed to diverge from it
        // (a headerless/replayed record may carry a synthetic test key); the
        // identity this row's `UNIQUE (source, external_id)` protects is the
        // one the bank itself assigned.
        let external_id = tx.reference.clone();
        let id = transaction_uuid(input.event.source, &external_id);

        let booking_date = parse_date("bookingDate", &tx.booking_date)?;
        let valuta_date = NaiveDate::parse_from_str(&tx.valuta_date, "%Y-%m-%d").ok();

        let amount = parse_decimal("amount.value", &tx.amount.value)?;
        let counterparty_name = pick_counterparty_name(&tx);
        let counterparty_iban = tx
            .creditor
            .as_ref()
            .and_then(|c| non_empty(&c.iban));
        let description = non_empty(&tx.remittance_info);

        Ok(TransactionRecord {
            id,
            account_id,
            source: input.event.source.to_string(),
            external_id,
            booking_date,
            valuta_date,
            booking_status: tx.booking_status,
            amount,
            currency: tx.amount.unit,
            counterparty_name,
            counterparty_iban,
            description,
            transaction_type: non_empty(&tx.transaction_type.key),
            raw_payload,
            origin: ORIGIN_SOURCE.to_string(),
            imported_at: input.event.imported_at,
            updated_at: input.event.imported_at,
            // TODO(WP3): call `labeling::normalize` here once it lands (WP2)
            // and wire the processor to recompute it on every upsert.
            counterparty_key: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kafka::envelope::SourceEvent;
    use chrono::{TimeZone, Utc};

    fn imported_at() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 6, 15, 8, 30, 0).unwrap()
    }

    fn input<'a>(
        source_account_id: Option<&'a str>,
        key: &'a str,
        payload: &'a [u8],
        comdirect_account_name: Option<&'a str>,
    ) -> MapperInput<'a> {
        MapperInput {
            event: SourceEvent {
                source: "comdirect",
                source_account_id,
                key,
                payload,
                imported_at: imported_at(),
            },
            comdirect_account_key: Some("0"),
            comdirect_account_name,
        }
    }

    const ACCOUNT_PAYLOAD: &[u8] = br#"{
        "iban": "DE11500105171234567890",
        "bic": "COBADEFFXXX",
        "accountId": "DE1053820100A1",
        "accountDisplayId": "1053820100",
        "accountType": {"text": "Girokonto"}
    }"#;

    #[test]
    fn maps_account_payload_with_label_from_header() {
        let input = input(None, "DE1053820100A1", ACCOUNT_PAYLOAD, Some("Main"));
        let account = ComdirectMapper.map_account(&input).unwrap();

        assert_eq!(account.source, "comdirect");
        assert_eq!(account.external_id, "DE1053820100A1");
        assert_eq!(account.display_id.as_deref(), Some("1053820100"));
        assert_eq!(account.account_type.as_deref(), Some("Girokonto"));
        assert_eq!(account.iban.as_deref(), Some("DE11500105171234567890"));
        assert_eq!(account.bic.as_deref(), Some("COBADEFFXXX"));
        assert_eq!(account.label.as_deref(), Some("Main"));
        assert_eq!(account.currency, "EUR");
        assert_eq!(account.origin, ORIGIN_SOURCE);
        assert_eq!(account.first_seen_at, imported_at());
        assert_eq!(account.updated_at, imported_at());
        assert_eq!(
            account.id,
            account_uuid("comdirect", "DE1053820100A1"),
            "account id must be deterministic from (source, external_id)"
        );
        assert!(account.raw_payload.is_some());
    }

    #[test]
    fn account_without_login_label_leaves_label_none() {
        let input = input(None, "DE1053820100A1", ACCOUNT_PAYLOAD, None);
        let account = ComdirectMapper.map_account(&input).unwrap();
        assert_eq!(account.label, None);
    }

    const BALANCE_PAYLOAD: &[u8] = br#"{"value": "1200.00", "unit": "EUR"}"#;

    #[test]
    fn maps_balance_payload_dated_from_imported_at() {
        let input = input(
            Some("DE1053820100A1"),
            "DE1053820100A1",
            BALANCE_PAYLOAD,
            Some("Main"),
        );
        let balance = ComdirectMapper.map_balance(&input).unwrap();

        assert_eq!(balance.amount, Decimal::from_str("1200.00").unwrap());
        assert_eq!(balance.currency, "EUR");
        assert_eq!(balance.balance_date, imported_at().date_naive());
        assert_eq!(balance.origin, ORIGIN_SOURCE);
        assert_eq!(
            balance.account_id,
            account_uuid("comdirect", "DE1053820100A1")
        );
        assert_eq!(
            balance.id,
            balance_uuid(balance.account_id, imported_at().date_naive())
        );
    }

    #[test]
    fn balance_without_source_account_id_is_skipped() {
        let input = input(None, "DE1053820100A1", BALANCE_PAYLOAD, Some("Main"));
        let err = ComdirectMapper.map_balance(&input).unwrap_err();
        assert_eq!(err, MapError::MissingSourceAccountId);
    }

    const SALARY_PAYLOAD: &[u8] = br#"{
        "reference": "ACC1-01-SALARY",
        "bookingStatus": "BOOKED",
        "bookingDate": "2024-01-02",
        "amount": {"value": "2500.00", "unit": "EUR"},
        "remitter": {"holderName": "ACME GmbH"},
        "deptor": null,
        "creditor": null,
        "valutaDate": "2024-01-02",
        "directDebitCreditorId": null,
        "directDebitMandateId": null,
        "endToEndReference": null,
        "newTransaction": false,
        "remittanceInfo": "Gehalt",
        "transactionType": {"key": "TRANSFER", "text": "\u00dcberweisungsgutschrift"}
    }"#;

    #[test]
    fn maps_transaction_payload_identity_from_reference_not_kafka_key() {
        let input = input(
            Some("DE1053820100A1"),
            "a-totally-different-kafka-key",
            SALARY_PAYLOAD,
            Some("Main"),
        );
        let tx = ComdirectMapper.map_transaction(&input).unwrap();

        assert_eq!(tx.external_id, "ACC1-01-SALARY");
        assert_eq!(tx.id, transaction_uuid("comdirect", "ACC1-01-SALARY"));
        assert_eq!(tx.booking_date, NaiveDate::from_ymd_opt(2024, 1, 2).unwrap());
        assert_eq!(tx.valuta_date, Some(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap()));
        assert_eq!(tx.amount, Decimal::from_str("2500.00").unwrap());
        assert_eq!(tx.currency, "EUR");
        assert_eq!(tx.counterparty_name.as_deref(), Some("ACME GmbH"));
        assert_eq!(tx.counterparty_iban, None);
        assert_eq!(tx.description.as_deref(), Some("Gehalt"));
        assert_eq!(tx.transaction_type.as_deref(), Some("TRANSFER"));
        assert_eq!(tx.booking_status, "BOOKED");
        assert_eq!(tx.origin, ORIGIN_SOURCE);
        assert_eq!(
            tx.account_id,
            account_uuid("comdirect", "DE1053820100A1")
        );
    }

    #[test]
    fn transaction_without_source_account_id_is_skipped() {
        let input = input(None, "ACC1-01-SALARY", SALARY_PAYLOAD, Some("Main"));
        let err = ComdirectMapper.map_transaction(&input).unwrap_err();
        assert_eq!(err, MapError::MissingSourceAccountId);
    }

    const RENT_PAYLOAD_NO_NAMES: &[u8] = br#"{
        "reference": "ACC1-RENT-NONAMES",
        "bookingStatus": "BOOKED",
        "bookingDate": "2024-02-01",
        "amount": {"value": "-900.00", "unit": "EUR"},
        "remitter": null,
        "deptor": null,
        "creditor": {"holderName": "", "iban": "", "bic": ""},
        "valutaDate": "2024-02-01",
        "directDebitCreditorId": null,
        "directDebitMandateId": null,
        "endToEndReference": null,
        "newTransaction": false,
        "remittanceInfo": "",
        "transactionType": {"key": "TRANSFER", "text": "Dauerauftrag"}
    }"#;

    #[test]
    fn missing_counterparty_and_description_become_none_not_empty_strings() {
        let input = input(
            Some("DE1053820100A1"),
            "ACC1-RENT-NONAMES",
            RENT_PAYLOAD_NO_NAMES,
            Some("Main"),
        );
        let tx = ComdirectMapper.map_transaction(&input).unwrap();

        assert_eq!(tx.counterparty_name, None);
        assert_eq!(tx.counterparty_iban, None);
        assert_eq!(tx.description, None);
    }

    const BAD_DATE_PAYLOAD: &[u8] = br#"{
        "reference": "ACC1-BAD-DATE",
        "bookingStatus": "BOOKED",
        "bookingDate": "not-a-date",
        "amount": {"value": "1.00", "unit": "EUR"},
        "remitter": null,
        "deptor": null,
        "creditor": null,
        "valutaDate": "2024-02-01",
        "directDebitCreditorId": null,
        "directDebitMandateId": null,
        "endToEndReference": null,
        "newTransaction": false,
        "remittanceInfo": "x",
        "transactionType": {"key": "TRANSFER", "text": "x"}
    }"#;

    #[test]
    fn unparseable_booking_date_fails_the_mapping() {
        let input = input(
            Some("DE1053820100A1"),
            "ACC1-BAD-DATE",
            BAD_DATE_PAYLOAD,
            Some("Main"),
        );
        let err = ComdirectMapper.map_transaction(&input).unwrap_err();
        assert!(matches!(err, MapError::UnparseableDate { field: "bookingDate", .. }));
    }
}
