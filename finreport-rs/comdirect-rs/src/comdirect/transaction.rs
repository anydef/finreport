use crate::comdirect::balance_model::Paging;
use crate::comdirect::raw::Raw;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
#[derive(Debug)]
pub enum TransactionsError {
    ResponseError,
}

pub struct TransactionsReq {
    pub account_uuid: String,
    pub page: i32,
    pub transaction_state: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Transaction {
    #[serde(rename = "reference")]
    pub reference: String,
    #[serde(rename = "bookingStatus")]
    pub booking_status: String,
    #[serde(rename = "bookingDate")]
    pub booking_date: String,
    #[serde(rename = "amount")]
    pub amount: Amount,
    #[serde(rename = "remitter")]
    pub remitter: Option<Remitter>,
    #[serde(rename = "deptor")]
    pub deptor: Option<String>,
    #[serde(rename = "creditor")]
    pub creditor: Option<Creditor>,
    #[serde(rename = "valutaDate")]
    pub valuta_date: String,
    #[serde(rename = "directDebitCreditorId")]
    pub direct_debit_creditor_id: Option<String>,
    #[serde(rename = "directDebitMandateId")]
    pub direct_debit_mandate_id: Option<String>,
    #[serde(rename = "endToEndReference")]
    pub end_to_end_reference: Option<String>,
    #[serde(rename = "newTransaction")]
    pub new_transaction: bool,
    #[serde(rename = "remittanceInfo", default)]
    pub remittance_info: String,
    #[serde(rename = "transactionType")]
    pub transaction_type: TransactionType,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Amount {
    #[serde(rename = "value")]
    pub value: String,
    #[serde(rename = "unit")]
    pub unit: String,
}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Remitter {
    /// Comdirect omits the holder name on some transaction types, so this
    /// defaults rather than failing the whole page of transactions.
    #[serde(rename = "holderName", default)]
    pub holder_name: String,
}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Creditor {
    /// All three are absent for creditors Comdirect reports without bank
    /// details (card payments, fees, internal bookings). They must default
    /// instead of being required: serde failing one record fails the entire
    /// transactions page, which aborts that account's whole import.
    #[serde(rename = "holderName", default)]
    pub holder_name: String,
    #[serde(default)]
    pub iban: String,
    #[serde(default)]
    bic: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TransactionType {
    pub key: String,
    pub text: String,
}
#[derive(Debug, Deserialize, Default)]
pub struct TransactionsResponse {
    pub paging: Paging,
    pub values: Vec<Transaction>,
}

/// Mirrors `TransactionsResponse`, but each element keeps the exact raw JSON
/// it came from alongside the parsed `Transaction`, for the Kafka path.
#[derive(Deserialize)]
struct TransactionsResponseShadow {
    paging: Paging,
    values: Vec<Box<RawValue>>,
}

pub struct TransactionsResponseRaw {
    pub paging: Paging,
    pub values: Vec<Raw<Transaction>>,
}

impl TransactionsResponseRaw {
    /// Parses straight from the raw response body — see
    /// `AccountsBalancesResponseRaw::from_json` for why this avoids
    /// `serde_json::to_string(&parsed)`.
    pub fn from_json(body: &str) -> serde_json::Result<Self> {
        let shadow: TransactionsResponseShadow = serde_json::from_str(body)?;
        let values = shadow
            .values
            .into_iter()
            .map(Raw::from_raw_value)
            .collect::<serde_json::Result<Vec<_>>>()?;
        Ok(TransactionsResponseRaw {
            paging: shadow.paging,
            values,
        })
    }
}

// --- Early-stop pagination ---------------------------------------------------
//
// The importer wants to stop paging once it reaches transactions it already
// has, instead of always walking every page. That's only safe if Comdirect
// returns BOOKED transactions newest-first — nothing in the REST docs or the
// Postman collection states that as a guarantee (the sample response in
// `docs/comdirect_rest_api.postman_collection.json` happens to be sorted that
// way, but a sample isn't a contract). So every page is checked for
// non-increasing booking dates before it's trusted; if a page ever violates
// that, early-stop is abandoned for the rest of the account and every
// remaining page is kept in full, same as the always-fetch-everything path.

/// A per-account resume point: the newest transaction already imported.
/// `last_reference` is preferred when present (Comdirect's transaction
/// `reference` is stable and exact); `last_booking_date` is the fallback.
#[derive(Debug, Clone, Default)]
pub struct ImportStop {
    pub last_reference: Option<String>,
    pub last_booking_date: Option<chrono::NaiveDate>,
}

/// Outcome of checking one page against an `ImportStop`.
#[derive(Debug)]
pub enum PageOutcome<T> {
    /// The stop point wasn't in this page; keep paging. Carries every
    /// transaction on the page.
    Continue(Vec<T>),
    /// The stop point was found; carries only the transactions newer than it.
    /// Paging should end here.
    Stop(Vec<T>),
}

/// Minimal view an early-stop pass needs from one paged record.
///
/// Implemented for both `Transaction` and `Raw<Transaction>` so
/// `filter_page_for_stop` — and the ordering guard it relies on — work
/// identically whether or not the raw JSON is carried alongside the parsed
/// struct. The Kafka dual-write needs `Raw<Transaction>`; plain imports use
/// `Transaction` directly.
pub trait StopMatchable {
    fn reference(&self) -> &str;
    fn transaction_booking_date(&self) -> Option<chrono::NaiveDate>;
}

impl StopMatchable for Transaction {
    fn reference(&self) -> &str {
        &self.reference
    }
    fn transaction_booking_date(&self) -> Option<chrono::NaiveDate> {
        self.booking_date.parse().ok()
    }
}

impl StopMatchable for Raw<Transaction> {
    fn reference(&self) -> &str {
        &self.parsed.reference
    }
    fn transaction_booking_date(&self) -> Option<chrono::NaiveDate> {
        self.parsed.booking_date.parse().ok()
    }
}

/// True if booking dates never increase moving through the page — the
/// newest-first shape early-stop depends on. Unparsable dates don't count as
/// a violation on their own; they just can't confirm one either.
fn page_is_sorted_newest_first<T: StopMatchable>(values: &[T]) -> bool {
    values.windows(2).all(|pair| {
        match (
            pair[0].transaction_booking_date(),
            pair[1].transaction_booking_date(),
        ) {
            (Some(a), Some(b)) => a >= b,
            _ => true,
        }
    })
}

fn reached_stop<T: StopMatchable>(tx: &T, stop: &ImportStop) -> bool {
    if let Some(want_reference) = &stop.last_reference {
        return tx.reference() == want_reference;
    }
    if let Some(stop_date) = stop.last_booking_date {
        if let Some(tx_date) = tx.transaction_booking_date() {
            return tx_date <= stop_date;
        }
    }
    false
}

/// Filters one already-fetched page against `stop`.
///
/// Returns `Err(values)` — handing the untouched page straight back — when
/// the page isn't sorted newest-first, so the caller can fall back to
/// keeping everything rather than risk truncating on an unordered page.
///
/// Generic over `StopMatchable` so the same logic serves both the typed-only
/// path (`Transaction`) and the Kafka dual-write path (`Raw<Transaction>`) —
/// no re-serialization involved either way, since `Raw<Transaction>` already
/// carries its raw bytes from parse time.
pub fn filter_page_for_stop<T: StopMatchable>(
    values: Vec<T>,
    stop: &ImportStop,
) -> Result<PageOutcome<T>, Vec<T>> {
    if !page_is_sorted_newest_first(&values) {
        return Err(values);
    }

    let mut kept = Vec::with_capacity(values.len());
    for tx in values {
        if reached_stop(&tx, stop) {
            return Ok(PageOutcome::Stop(kept));
        }
        kept.push(tx);
    }
    Ok(PageOutcome::Continue(kept))
}

#[cfg(test)]
mod test {
    use super::{
        filter_page_for_stop, ImportStop, PageOutcome, Transaction, TransactionsResponse,
        TransactionsResponseRaw,
    };
    use crate::comdirect::raw::Raw;

    /// Builds a minimal, otherwise-valid transaction JSON string with the
    /// given reference/booking date, plus an unmodeled marker field so raw
    /// tests can prove the captured bytes are the exact original slice.
    fn tx_json(reference: &str, booking_date: &str) -> String {
        format!(
            r#"{{
                "reference": "{reference}",
                "bookingStatus": "BOOKED",
                "bookingDate": "{booking_date}",
                "amount": {{ "value": "-1.00", "unit": "EUR" }},
                "valutaDate": "{booking_date}",
                "newTransaction": false,
                "somethingWeDontModel": "marker-{reference}",
                "transactionType": {{ "key": "TRANSFER", "text": "Transfer" }}
            }}"#
        )
    }

    /// Parses a `Raw<Transaction>` straight from `tx_json`'s bytes — the
    /// same path `TransactionsResponseRaw::from_json` uses for one array
    /// element (capture as `RawValue`, then parse `Transaction` from that
    /// same captured slice). No detour through a generic `serde_json::Value`,
    /// which would reorder/reformat the bytes and defeat the point of the
    /// byte-identity test below.
    fn raw_tx(reference: &str, booking_date: &str) -> Raw<Transaction> {
        let json = tx_json(reference, booking_date);
        let raw: Box<serde_json::value::RawValue> =
            serde_json::from_str(&json).expect("raw capture must succeed");
        Raw::from_raw_value(raw).expect("minimal transaction JSON must parse")
    }

    /// Builds a minimal, otherwise-valid transaction with the given
    /// reference/booking date, for pagination tests that don't care about
    /// the rest of the fields.
    fn tx(reference: &str, booking_date: &str) -> Transaction {
        let json = format!(
            r#"{{
                "reference": "{reference}",
                "bookingStatus": "BOOKED",
                "bookingDate": "{booking_date}",
                "amount": {{ "value": "-1.00", "unit": "EUR" }},
                "valutaDate": "{booking_date}",
                "newTransaction": false,
                "transactionType": {{ "key": "TRANSFER", "text": "Transfer" }}
            }}"#
        );
        serde_json::from_str(&json).expect("minimal transaction JSON must parse")
    }

    /// A creditor with no bank details — the shape that aborted account 1's
    /// import with `missing field \`iban\``. One such record must not cost us
    /// the rest of the page.
    #[test]
    fn transactions_parse_when_creditor_has_no_iban() {
        let json = r#"{
            "paging": { "index": 0, "matches": 2 },
            "values": [
                {
                    "reference": "ref-1",
                    "bookingStatus": "BOOKED",
                    "bookingDate": "2026-08-20",
                    "amount": { "value": "-12.34", "unit": "EUR" },
                    "creditor": { "holderName": "Some Shop" },
                    "valutaDate": "2026-08-20",
                    "newTransaction": false,
                    "transactionType": { "key": "DIRECT_DEBIT", "text": "Lastschrift" }
                },
                {
                    "reference": "ref-2",
                    "bookingStatus": "BOOKED",
                    "bookingDate": "2026-08-20",
                    "amount": { "value": "100.00", "unit": "EUR" },
                    "remitter": { "holderName": "Someone" },
                    "creditor": { "holderName": "Me", "iban": "DE00", "bic": "XX" },
                    "valutaDate": "2026-08-20",
                    "newTransaction": true,
                    "remittanceInfo": "salary",
                    "transactionType": { "key": "TRANSFER", "text": "Uberweisung" }
                }
            ]
        }"#;

        let response: TransactionsResponse =
            serde_json::from_str(json).expect("page with a bank-detail-less creditor must parse");

        assert_eq!(response.values.len(), 2);
        let first = &response.values[0];
        assert_eq!(first.creditor.as_ref().unwrap().iban, "");
        // Absent remittanceInfo defaults too, rather than failing the record.
        assert_eq!(first.remittance_info, "");
        assert_eq!(response.values[1].remittance_info, "salary");
    }

    /// The Kafka path re-publishes Comdirect's JSON byte-for-byte, so the
    /// captured `raw` must survive untouched even for a field our `Transaction`
    /// struct doesn't model at all (here, a made-up `somethingWeDontModel`).
    /// A round-trip through `serde_json::to_string(&parsed)` would drop it.
    #[test]
    fn raw_passthrough_preserves_exact_bytes_including_unmodeled_fields() {
        let json = r#"{
            "paging": { "index": 0, "matches": 1 },
            "values": [
                {
                    "reference": "ref-1",
                    "bookingStatus": "BOOKED",
                    "bookingDate": "2026-08-20",
                    "amount": { "value": "-12.34", "unit": "EUR" },
                    "creditor": { "holderName": "Some Shop" },
                    "valutaDate": "2026-08-20",
                    "newTransaction": false,
                    "somethingWeDontModel": { "nested": [1, 2, 3] },
                    "transactionType": { "key": "DIRECT_DEBIT", "text": "Lastschrift" }
                }
            ]
        }"#;

        let response =
            TransactionsResponseRaw::from_json(json).expect("raw response must parse");

        assert_eq!(response.values.len(), 1);
        let record = &response.values[0];
        assert_eq!(record.parsed.reference, "ref-1");

        // The raw bytes must contain the unmodeled field verbatim...
        assert!(record.raw.get().contains("somethingWeDontModel"));
        // Whitespace preserved too — this is the exact original slice, not a
        // reformat via a generic JSON value.
        assert!(record.raw.get().contains("\"nested\": [1, 2, 3]"));
        // ...and round-tripping the raw bytes through a generic JSON value
        // must equal round-tripping the *original* record's slice — i.e. raw
        // really is that exact record, not the whole array or a reformat.
        let expected: serde_json::Value = serde_json::from_str(
            r#"{
                "reference": "ref-1",
                "bookingStatus": "BOOKED",
                "bookingDate": "2026-08-20",
                "amount": { "value": "-12.34", "unit": "EUR" },
                "creditor": { "holderName": "Some Shop" },
                "valutaDate": "2026-08-20",
                "newTransaction": false,
                "somethingWeDontModel": { "nested": [1, 2, 3] },
                "transactionType": { "key": "DIRECT_DEBIT", "text": "Lastschrift" }
            }"#,
        )
        .unwrap();
        let actual: serde_json::Value = serde_json::from_str(record.raw.get()).unwrap();
        assert_eq!(actual, expected);
    }

    /// Early-stop must return only the transactions newer than the resume
    /// point, matching on `reference` when the stop point provides one.
    #[test]
    fn early_stop_returns_only_records_newer_than_stop_point() {
        let page = vec![
            tx("ref-3", "2026-08-20"),
            tx("ref-2", "2026-08-19"),
            tx("ref-1", "2026-08-18"), // already imported: this is the stop point
            tx("ref-0", "2026-08-17"),
        ];
        let stop = ImportStop {
            last_reference: Some("ref-1".to_string()),
            last_booking_date: None,
        };

        let outcome = filter_page_for_stop(page, &stop).expect("page is sorted newest-first");

        match outcome {
            PageOutcome::Stop(kept) => {
                let refs: Vec<&str> = kept.iter().map(|t| t.reference.as_str()).collect();
                assert_eq!(refs, vec!["ref-3", "ref-2"]);
            }
            PageOutcome::Continue(_) => panic!("expected the stop point to be found in this page"),
        }
    }

    /// Falls back to `last_booking_date` when the stop point has no
    /// reference (e.g. bootstrapping from an account that only recorded a
    /// balance date so far).
    #[test]
    fn early_stop_falls_back_to_booking_date_when_no_reference() {
        let page = vec![tx("ref-2", "2026-08-20"), tx("ref-1", "2026-08-18")];
        let stop = ImportStop {
            last_reference: None,
            last_booking_date: "2026-08-18".parse().ok(),
        };

        let outcome = filter_page_for_stop(page, &stop).expect("page is sorted newest-first");
        match outcome {
            PageOutcome::Stop(kept) => {
                assert_eq!(kept.len(), 1);
                assert_eq!(kept[0].reference, "ref-2");
            }
            PageOutcome::Continue(_) => panic!("expected the stop point to be found in this page"),
        }
    }

    /// If a page isn't sorted newest-first, the ordering assumption early-stop
    /// depends on doesn't hold — the guard must hand the whole page straight
    /// back (`Err`) rather than let the caller truncate on unreliable order.
    #[test]
    fn out_of_order_page_falls_back_to_keeping_everything() {
        let page = vec![
            tx("ref-1", "2026-08-18"),
            tx("ref-2", "2026-08-20"), // out of order: newer than the previous record
            tx("ref-3", "2026-08-19"),
        ];
        let stop = ImportStop {
            last_reference: Some("ref-1".to_string()),
            last_booking_date: None,
        };

        let result = filter_page_for_stop(page, &stop);
        match result {
            Err(untouched) => {
                let refs: Vec<&str> = untouched.iter().map(|t| t.reference.as_str()).collect();
                // Nothing truncated: every record from the page comes back,
                // including the one that would have matched the stop point.
                assert_eq!(refs, vec!["ref-1", "ref-2", "ref-3"]);
            }
            Ok(_) => panic!("out-of-order page must not be trusted for early-stop"),
        }
    }

    /// The Kafka dual-write path needs early-stop and raw passthrough at the
    /// same time: `filter_page_for_stop` must work over `Raw<Transaction>`
    /// exactly like it does over `Transaction` (same matching, same kept
    /// set), while every kept record's raw JSON stays byte-identical to what
    /// went in — including a field `Transaction` doesn't model.
    #[test]
    fn early_stop_over_raw_transactions_keeps_only_newer_records_with_untouched_bytes() {
        let page = vec![
            raw_tx("ref-3", "2026-08-20"),
            raw_tx("ref-2", "2026-08-19"),
            raw_tx("ref-1", "2026-08-18"), // already imported: the stop point
            raw_tx("ref-0", "2026-08-17"),
        ];
        let stop = ImportStop {
            last_reference: Some("ref-1".to_string()),
            last_booking_date: None,
        };

        let outcome = filter_page_for_stop(page, &stop).expect("page is sorted newest-first");

        match outcome {
            PageOutcome::Stop(kept) => {
                let refs: Vec<&str> = kept.iter().map(|r| r.parsed.reference.as_str()).collect();
                assert_eq!(refs, vec!["ref-3", "ref-2"]);

                for (record, reference) in kept.iter().zip(["ref-3", "ref-2"]) {
                    // Byte-identical to the exact input for that record: the
                    // unmodeled marker field survives, and the raw slice
                    // parses back to precisely the original document.
                    let expected: serde_json::Value = serde_json::from_str(&tx_json(
                        reference,
                        if reference == "ref-3" {
                            "2026-08-20"
                        } else {
                            "2026-08-19"
                        },
                    ))
                    .unwrap();
                    let actual: serde_json::Value =
                        serde_json::from_str(record.raw.get()).unwrap();
                    assert_eq!(actual, expected);
                    assert!(record
                        .raw
                        .get()
                        .contains(&format!("marker-{reference}")));
                }
            }
            PageOutcome::Continue(_) => panic!("expected the stop point to be found in this page"),
        }
    }
}
