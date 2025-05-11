use crate::comdirect::balance_model::Paging;
use serde::Deserialize;
#[derive(Debug)]
pub enum TransactionsError {
    ResponseError,
}

pub struct TransactionsReq {
    pub account_uuid: String,
    pub page: i32,
    pub transaction_state: String,
}

#[derive(Debug, Deserialize)]
pub struct Transaction {
    #[serde(rename = "reference")]
    pub reference: String,
    // #[serde(rename = "bookingStatus")]
    // pub booking_status: String,
    // #[serde(rename = "bookingDate")]
    // pub booking_date: String,
    // #[serde(rename = "amount")]
    // pub amount: Amount,
    // #[serde(rename = "remitter")]
    // pub remitter: Option<Remitter>,
    // #[serde(rename = "deptor")]
    // pub deptor: Option<String>,
    // #[serde(rename = "creditor")]
    // pub creditor: Option<String>,
    // #[serde(rename = "valutaDate")]
    // pub valuta_date: String,
    // #[serde(rename = "directDebitCreditorId")]
    // pub direct_debit_creditor_id: Option<String>,
    // #[serde(rename = "directDebitMandateId")]
    // pub direct_debit_mandate_id: Option<String>,
    // #[serde(rename = "endToEndReference")]
    // pub end_to_end_reference: Option<String>,
    // #[serde(rename = "newTransaction")]
    // pub new_transaction: bool,
    // #[serde(rename = "remittanceInfo")]
    // pub remittance_info: String,
    // #[serde(rename = "transactionType")]
    // pub transaction_type: TransactionType,
}

#[derive(Debug, Deserialize)]
pub struct Amount {
    #[serde(rename = "value")]
    pub value: String,
    #[serde(rename = "unit")]
    pub unit: String,
}

#[derive(Debug, Deserialize)]
pub struct Remitter {
    #[serde(rename = "holderName")]
    pub holder_name: String,
}

#[derive(Debug, Deserialize)]
pub struct TransactionType {
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "text")]
    pub text: String,
}
#[derive(Debug, Deserialize, Default)]
pub struct TransactionsResponse {
    pub paging: Paging,
    pub values: Vec<Transaction>,
}



// pub async fn transaction(
//     session: &(impl TokenAware + ClientSession),
//     transactions_req: TransactionsReq,
// ) -> Result<TransactionsResponse, TransactionsError> {
//     let TransactionsReq {
//         account_uuid,
//         page,
//         transaction_state,
//     } = transactions_req;
//     let url = format!(
//         "{}/banking/v1/accounts/{}/transactions?transactionState={}&paging-first={}",
//         session.url(),
//         account_uuid,
//         transaction_state,
//         page
//     );
//
//     let response = session
//         .client()
//         .get(&url)
//         .header(ACCEPT, "application/json")
//         .header(CONTENT_TYPE, "application/json")
//         .header("Authorization", format!("Bearer {}", session.access_token().unwrap_or_default()))
//         .header("x-http-request-info", session.info_header())
//
//         .send().await;
//
//     match response {
//         Ok(res) => {
//             if res.status() == StatusCode::OK {
//                 let transactions: TransactionsResponse = res.json().await.unwrap();
//                 Ok(transactions)
//             } else {
//                 Err(TransactionsError::ResponseError)
//             }
//         }
//         Err(e) => {
//             Err(TransactionsError::ResponseError)
//         }
//     }
//
//     // Ok(TransactionsResponse {})
// }
