//! `C24Mapper`: `(c24, source)`. Maps the two C24 wire records
//! ([`crate::kafka::c24`]) onto the normalized account/transaction rows.
//!
//! # No cell value in an error
//!
//! Every C24 cell is bank data, and the C24 contract is that no error message
//! or log line carries one. `MapError::UnparseableDate` / `UnparseableAmount`
//! carry the offending value and `MapError: Display` prints it, and a
//! `serde_json::Error`'s text can quote the value it choked on. So this mapper
//! never constructs those variants and never forwards a serde message: every
//! failure is `MapError::InvalidPayload` with a static string naming only the
//! record kind.

use serde_json::Value;

use crate::kafka::c24::{C24AccountRecord, C24TransactionRecord, SOURCE_C24};
use crate::kafka::envelope::{account_uuid, transaction_uuid};
use crate::projection::mapper::{MapperInput, SourceMapper};
use crate::projection::records::{
    AccountRecord, BalanceRecord, MapError, TransactionRecord, ORIGIN_SOURCE,
};

pub struct C24Mapper;

const BAD_ACCOUNT: &str = "malformed c24 account record";
const BAD_TRANSACTION: &str = "malformed c24 transaction record";

fn parse<T: serde::de::DeserializeOwned>(
    payload: &[u8],
    reason: &'static str,
) -> Result<(T, Value), MapError> {
    let record =
        serde_json::from_slice(payload).map_err(|_| MapError::InvalidPayload(reason.to_string()))?;
    let raw =
        serde_json::from_slice(payload).map_err(|_| MapError::InvalidPayload(reason.to_string()))?;
    Ok((record, raw))
}

impl SourceMapper for C24Mapper {
    fn source(&self) -> &'static str {
        SOURCE_C24
    }

    fn map_account(&self, input: &MapperInput<'_>) -> Result<AccountRecord, MapError> {
        let (rec, raw): (C24AccountRecord, Value) = parse(input.event.payload, BAD_ACCOUNT)?;
        Ok(AccountRecord {
            id: account_uuid(input.event.source, &rec.external_id),
            source: input.event.source.to_string(),
            display_id: None,
            account_type: None,
            iban: rec.iban,
            bic: None,
            institute: None,
            label: rec.label,
            currency: "EUR".to_string(),
            external_id: rec.external_id,
            raw_payload: Some(raw),
            origin: ORIGIN_SOURCE.to_string(),
            first_seen_at: input.event.imported_at,
            updated_at: input.event.imported_at,
        })
    }

    /// The C24 export has no balance column: there is nothing to map, and
    /// inventing one would put a made-up number in the read model.
    fn map_balance(&self, _input: &MapperInput<'_>) -> Result<BalanceRecord, MapError> {
        Err(MapError::InvalidPayload(
            "c24 has no balance records".to_string(),
        ))
    }

    fn map_transaction(&self, input: &MapperInput<'_>) -> Result<TransactionRecord, MapError> {
        let source_account_id = input
            .event
            .source_account_id
            .ok_or(MapError::MissingSourceAccountId)?;
        let (rec, raw): (C24TransactionRecord, Value) =
            parse(input.event.payload, BAD_TRANSACTION)?;
        Ok(TransactionRecord {
            id: transaction_uuid(input.event.source, &rec.external_id),
            account_id: account_uuid(input.event.source, source_account_id),
            source: input.event.source.to_string(),
            external_id: rec.external_id,
            booking_date: rec.booking_date,
            valuta_date: rec.valuta_date,
            booking_status: rec.booking_status,
            amount: rec.amount,
            currency: rec.currency,
            counterparty_name: rec.counterparty_name,
            counterparty_iban: rec.counterparty_iban,
            description: rec.description,
            transaction_type: Some(rec.transaction_type).filter(|t| !t.is_empty()),
            raw_payload: raw,
            origin: ORIGIN_SOURCE.to_string(),
            imported_at: input.event.imported_at,
            updated_at: input.event.imported_at,
            // Filled by the labeler's processor, as for every source.
            counterparty_key: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kafka::c24::test_support::{card_row, transfer_row};
    use crate::kafka::envelope::SourceEvent;
    use crate::projection::mapper::MapperRegistry;
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;

    fn input<'a>(
        source_account_id: Option<&'a str>,
        key: &'a str,
        payload: &'a [u8],
    ) -> MapperInput<'a> {
        MapperInput {
            event: SourceEvent {
                source: "c24",
                source_account_id,
                key,
                payload,
                imported_at: Utc.with_ymd_and_hms(2026, 3, 6, 9, 0, 0).unwrap(),
            },
            comdirect_account_key: None,
            comdirect_account_name: None,
        }
    }

    #[test]
    fn maps_a_transfer_to_a_transaction_record() {
        let rec = C24TransactionRecord::from_row(&transfer_row());
        let bytes = serde_json::to_vec(&rec).unwrap();
        let out = C24Mapper
            .map_transaction(&input(Some("0000012345"), &rec.external_id, &bytes))
            .unwrap();

        assert_eq!(out.source, "c24");
        assert_eq!(out.external_id, rec.external_id);
        assert_eq!(out.id, transaction_uuid("c24", &rec.external_id));
        assert_eq!(out.account_id, account_uuid("c24", "0000012345"));
        assert_eq!(out.amount, Decimal::new(150000, 2));
        assert_eq!(out.currency, "EUR");
        assert_eq!(out.booking_status, "BOOKED");
        assert_eq!(out.counterparty_name.as_deref(), Some("Max Mustermann"));
        assert_eq!(out.counterparty_iban.as_deref(), Some("DE00000000000000000000"));
        assert_eq!(out.description.as_deref(), Some("Miete Maerz"));
        assert_eq!(out.transaction_type.as_deref(), Some("Echtzeit-Überweisung"));
        assert_eq!(out.valuta_date, None);
        assert_eq!(out.origin, "source");
        assert_eq!(out.raw_payload["c24_category"], Value::Null);
    }

    #[test]
    fn card_category_lands_in_raw_payload_only() {
        let rec = C24TransactionRecord::from_row(&card_row());
        let bytes = serde_json::to_vec(&rec).unwrap();
        let out = C24Mapper
            .map_transaction(&input(Some("0000012345"), &rec.external_id, &bytes))
            .unwrap();
        assert_eq!(out.raw_payload["c24_category"], "Essen");
        assert_eq!(out.description, None);
    }

    #[test]
    fn maps_an_account_record() {
        let rec = C24AccountRecord::from_row(&card_row());
        let bytes = serde_json::to_vec(&rec).unwrap();
        let out = C24Mapper
            .map_account(&input(None, &rec.external_id, &bytes))
            .unwrap();
        assert_eq!(out.id, account_uuid("c24", "0000012345"));
        assert_eq!(out.external_id, "0000012345");
        assert_eq!(out.label.as_deref(), Some("Testkonto"));
        assert_eq!(out.iban, None);
        assert_eq!(out.origin, "source");
    }

    #[test]
    fn transaction_without_source_account_id_is_rejected() {
        let rec = C24TransactionRecord::from_row(&card_row());
        let bytes = serde_json::to_vec(&rec).unwrap();
        assert_eq!(
            C24Mapper
                .map_transaction(&input(None, &rec.external_id, &bytes))
                .unwrap_err(),
            MapError::MissingSourceAccountId
        );
    }

    #[test]
    fn balance_is_refused_not_fabricated() {
        let err = C24Mapper
            .map_balance(&input(Some("0000012345"), "k", b"{}"))
            .unwrap_err();
        assert!(matches!(err, MapError::InvalidPayload(_)));
    }

    #[test]
    fn errors_never_carry_a_payload_value() {
        // Every value here is bank data. Each is malformed or surrounded by
        // malformed fields so serde has plenty of chances to quote one.
        let bad = br#"{"schema_version":1,"external_id":"SECRET-EXT-ID",
            "booking_date":"SECRET-DATE","valuta_date":null,"booking_status":"BOOKED",
            "amount":"SECRET-AMOUNT","currency":"EUR",
            "counterparty_name":"SECRET-PAYEE","counterparty_iban":"DE99SECRETIBAN",
            "transaction_type":"SECRET-TYPE","description":"SECRET-PURPOSE",
            "card_used_at":null,"c24_category":null,"c24_subcategory":null}"#;
        let secrets = [
            "SECRET-EXT-ID",
            "SECRET-DATE",
            "SECRET-AMOUNT",
            "SECRET-PAYEE",
            "DE99SECRETIBAN",
            "SECRET-TYPE",
            "SECRET-PURPOSE",
        ];

        let tx = C24Mapper
            .map_transaction(&input(Some("0000012345"), "k", bad))
            .unwrap_err();
        let acct = C24Mapper
            .map_account(&input(None, "k", br#"{"schema_version":"SECRET-AMOUNT"}"#))
            .unwrap_err();
        let truncated = C24Mapper
            .map_transaction(&input(Some("0000012345"), "k", &bad[..60]))
            .unwrap_err();

        for err in [tx, acct, truncated] {
            assert!(matches!(err, MapError::InvalidPayload(_)));
            let text = err.to_string();
            for secret in secrets {
                assert!(!text.contains(secret), "error leaked {secret}: {text}");
            }
        }
    }

    #[test]
    fn registry_resolves_c24_source() {
        let registry = MapperRegistry::with_default_mappers();
        assert!(registry.resolve("c24", "source").is_some());
        assert!(registry.resolve("c24", "legacy-backfill").is_none());
    }
}
