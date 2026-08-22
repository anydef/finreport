use crate::comdirect::balance_model::Paging;
use serde::{Deserialize, Serialize};
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
#[cfg(test)]
mod test {
    use super::TransactionsResponse;

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
}
