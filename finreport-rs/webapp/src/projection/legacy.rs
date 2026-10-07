//! `LegacyMapper`: `(comdirect, legacy-backfill)` (§2.4, §2.8).
//!
//! The payload for these records is **our own reconstruction**, not a bank
//! payload — the JSON shape mirrors the renamed `legacy_account` /
//! `legacy_account_balance` / `legacy_account_transactions` columns, because
//! that is literally where `legacy-backfill` (WP1) reads it from. Every
//! record this mapper produces carries `origin = "legacy"` (§2.8's spelling
//! difference: the *header* is `legacy-backfill`, the *column* is `legacy`).

use chrono::NaiveDate;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;

use crate::kafka::envelope::{account_uuid, balance_uuid, transaction_uuid};
use crate::projection::mapper::{MapperInput, SourceMapper};
use crate::projection::records::{
    AccountRecord, BalanceRecord, MapError, TransactionRecord, ORIGIN_LEGACY,
};

pub struct LegacyMapper;

/// Mirrors the renamed `legacy_account` columns (§3).
#[derive(Deserialize)]
struct LegacyAccount {
    account_id: String,
    display_id: Option<String>,
    account_type: Option<String>,
    iban: Option<String>,
    bic: Option<String>,
    institute: Option<String>,
    account_name: Option<String>,
}

/// Mirrors the renamed `legacy_account_balance` columns (§3).
#[derive(Deserialize)]
struct LegacyBalance {
    amount: f64,
    date: String,
}

/// Mirrors the renamed `legacy_account_transactions` columns (§3) — same
/// field names as `webapp/fixtures/payloads/legacy/transaction-0001.json`.
#[derive(Deserialize)]
struct LegacyTransaction {
    reference: String,
    booking_status: String,
    booking_date: String,
    amount: f64,
    remitter: String,
    deptor: String,
    creditor: String,
    remittance_info: String,
    transaction_type: String,
}

fn parse_json(payload: &[u8]) -> Result<Value, MapError> {
    serde_json::from_slice(payload).map_err(|e| MapError::InvalidPayload(e.to_string()))
}

fn parse_decimal_f64(field: &'static str, value: f64) -> Result<Decimal, MapError> {
    Decimal::from_f64(value).ok_or(MapError::UnparseableAmount {
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

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn require_source_account_id<'a>(input: &MapperInput<'a>) -> Result<&'a str, MapError> {
    input
        .event
        .source_account_id
        .ok_or(MapError::MissingSourceAccountId)
}

impl SourceMapper for LegacyMapper {
    fn source(&self) -> &'static str {
        "comdirect"
    }

    fn map_account(&self, input: &MapperInput<'_>) -> Result<AccountRecord, MapError> {
        let account: LegacyAccount = serde_json::from_slice(input.event.payload)
            .map_err(|e| MapError::InvalidPayload(e.to_string()))?;
        let raw_payload = parse_json(input.event.payload)?;

        let id = account_uuid(input.event.source, &account.account_id);

        Ok(AccountRecord {
            id,
            source: input.event.source.to_string(),
            external_id: account.account_id,
            display_id: account.display_id,
            account_type: account.account_type,
            iban: account.iban,
            bic: account.bic,
            institute: account.institute,
            label: account
                .account_name
                .or_else(|| input.comdirect_account_name.map(str::to_string)),
            currency: "EUR".to_string(),
            raw_payload: Some(raw_payload),
            origin: ORIGIN_LEGACY.to_string(),
            first_seen_at: input.event.imported_at,
            updated_at: input.event.imported_at,
        })
    }

    fn map_balance(&self, input: &MapperInput<'_>) -> Result<BalanceRecord, MapError> {
        let source_account_id = require_source_account_id(input)?;
        let balance: LegacyBalance = serde_json::from_slice(input.event.payload)
            .map_err(|e| MapError::InvalidPayload(e.to_string()))?;
        let raw_payload = parse_json(input.event.payload)?;

        let account_id = account_uuid(input.event.source, source_account_id);
        let amount = parse_decimal_f64("amount", balance.amount)?;
        // Reconstructed from the legacy row's own `date` column, never
        // `imported_at` (§2.5) — unlike the live Comdirect endpoint, the old
        // schema always had a real date for each balance observation.
        let balance_date = parse_date("date", &balance.date)?;
        let id = balance_uuid(account_id, balance_date);

        Ok(BalanceRecord {
            id,
            account_id,
            balance_date,
            amount,
            currency: "EUR".to_string(),
            raw_payload: Some(raw_payload),
            origin: ORIGIN_LEGACY.to_string(),
            observed_at: input.event.imported_at,
        })
    }

    fn map_transaction(&self, input: &MapperInput<'_>) -> Result<TransactionRecord, MapError> {
        let source_account_id = require_source_account_id(input)?;
        let tx: LegacyTransaction = serde_json::from_slice(input.event.payload)
            .map_err(|e| MapError::InvalidPayload(e.to_string()))?;
        let raw_payload = parse_json(input.event.payload)?;

        let account_id = account_uuid(input.event.source, source_account_id);
        let external_id = tx.reference.clone();
        let id = transaction_uuid(input.event.source, &external_id);

        let booking_date = parse_date("booking_date", &tx.booking_date)?;
        let amount = parse_decimal_f64("amount", tx.amount)?;

        let counterparty_name = non_empty(&tx.creditor)
            .or_else(|| non_empty(&tx.remitter))
            .or_else(|| non_empty(&tx.deptor));

        Ok(TransactionRecord {
            id,
            account_id,
            source: input.event.source.to_string(),
            external_id,
            booking_date,
            // The legacy schema never recorded a separate valuta date.
            valuta_date: None,
            booking_status: tx.booking_status,
            amount,
            currency: "EUR".to_string(),
            counterparty_name,
            // The legacy schema never recorded the counterparty's IBAN.
            counterparty_iban: None,
            description: non_empty(&tx.remittance_info),
            transaction_type: non_empty(&tx.transaction_type),
            raw_payload,
            origin: ORIGIN_LEGACY.to_string(),
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
        Utc.with_ymd_and_hms(2023, 11, 15, 20, 0, 0).unwrap()
    }

    fn input<'a>(source_account_id: Option<&'a str>, key: &'a str, payload: &'a [u8]) -> MapperInput<'a> {
        MapperInput {
            event: SourceEvent {
                source: "comdirect",
                source_account_id,
                key,
                payload,
                imported_at: imported_at(),
            },
            comdirect_account_key: None,
            comdirect_account_name: None,
        }
    }

    const TRANSACTION_PAYLOAD: &[u8] = br#"{
        "reference": "LEGACY-0001",
        "account_id": "DE1053820100A1",
        "booking_status": "BOOKED",
        "booking_date": "2023-11-15",
        "amount": -54.3,
        "remitter": "",
        "deptor": "",
        "creditor": "Alte Apotheke",
        "creditor_id": "",
        "creditor_mandate_id": "",
        "remittance_info": "Rezept",
        "transaction_type": "CARD_PAYMENT"
    }"#;

    #[test]
    fn maps_reconstructed_transaction_with_legacy_origin() {
        let input = input(
            Some("DE1053820100A1"),
            "LEGACY-0001",
            TRANSACTION_PAYLOAD,
        );
        let tx = LegacyMapper.map_transaction(&input).unwrap();

        assert_eq!(tx.external_id, "LEGACY-0001");
        assert_eq!(tx.origin, ORIGIN_LEGACY);
        assert_eq!(tx.booking_date, NaiveDate::from_ymd_opt(2023, 11, 15).unwrap());
        assert_eq!(tx.amount, Decimal::from_f64(-54.3).unwrap());
        assert_eq!(tx.counterparty_name.as_deref(), Some("Alte Apotheke"));
        assert_eq!(tx.description.as_deref(), Some("Rezept"));
        assert_eq!(tx.transaction_type.as_deref(), Some("CARD_PAYMENT"));
        assert_eq!(tx.valuta_date, None);
        assert_eq!(tx.counterparty_iban, None);
        assert_eq!(
            tx.account_id,
            account_uuid("comdirect", "DE1053820100A1")
        );
        assert_eq!(tx.id, transaction_uuid("comdirect", "LEGACY-0001"));
    }

    #[test]
    fn transaction_missing_source_account_id_is_skipped() {
        let input = input(None, "LEGACY-0001", TRANSACTION_PAYLOAD);
        let err = LegacyMapper.map_transaction(&input).unwrap_err();
        assert_eq!(err, MapError::MissingSourceAccountId);
    }

    const ACCOUNT_PAYLOAD: &[u8] = br#"{
        "account_id": "DE1053820100A1",
        "display_id": "1053820100",
        "account_type": "Girokonto",
        "iban": "DE11500105171234567890",
        "bic": "COBADEFFXXX",
        "institute": "comdirect",
        "account_name": "Main"
    }"#;

    #[test]
    fn maps_reconstructed_account_with_legacy_origin() {
        let input = input(None, "DE1053820100A1", ACCOUNT_PAYLOAD);
        let account = LegacyMapper.map_account(&input).unwrap();

        assert_eq!(account.external_id, "DE1053820100A1");
        assert_eq!(account.origin, ORIGIN_LEGACY);
        assert_eq!(account.label.as_deref(), Some("Main"));
        assert_eq!(
            account.id,
            account_uuid("comdirect", "DE1053820100A1")
        );
    }

    const BALANCE_PAYLOAD: &[u8] = br#"{
        "amount": -1234.56,
        "date": "2024-01-31",
        "account_id": "DE1053820100A1"
    }"#;

    #[test]
    fn maps_reconstructed_balance_dated_from_its_own_date_column() {
        let input = input(Some("DE1053820100A1"), "DE1053820100A1", BALANCE_PAYLOAD);
        let balance = LegacyMapper.map_balance(&input).unwrap();

        assert_eq!(balance.origin, ORIGIN_LEGACY);
        assert_eq!(balance.balance_date, NaiveDate::from_ymd_opt(2024, 1, 31).unwrap());
        assert_eq!(balance.amount, Decimal::from_f64(-1234.56).unwrap());
        assert_eq!(
            balance.account_id,
            account_uuid("comdirect", "DE1053820100A1")
        );
    }

    #[test]
    fn balance_missing_source_account_id_is_skipped() {
        let input = input(None, "DE1053820100A1", BALANCE_PAYLOAD);
        let err = LegacyMapper.map_balance(&input).unwrap_err();
        assert_eq!(err, MapError::MissingSourceAccountId);
    }
}
