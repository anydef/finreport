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
    #[serde(rename = "remittanceInfo")]
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

#[derive(Debug, Deserialize, Serialize)]
pub struct Remitter {
    #[serde(rename = "holderName")]
    pub holder_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Creditor {
    #[serde(rename = "holderName")]
    holder_name: String,
    iban: String,
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