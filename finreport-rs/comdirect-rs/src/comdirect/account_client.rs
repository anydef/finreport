use crate::comdirect::balance_model::AccountsBalancesResponse;
use crate::comdirect::session_client::HttpRequestInfoHeader;
use crate::comdirect::session_client::Session;
use crate::comdirect::transaction::TransactionsResponse;
use crate::comdirect::utils::request_id;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::StatusCode;
use reqwest_middleware::ClientWithMiddleware;
use serde::Deserialize;
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[derive(Debug)]
pub enum AccountClientError {
    Unauthorized,
    Unknown,
}

impl Display for AccountClientError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Client error: {:?}", self)
    }
}

impl std::error::Error for AccountClientError {}

impl From<reqwest_middleware::Error> for AccountClientError {
    fn from(value: reqwest_middleware::Error) -> Self {
        match value {
            reqwest_middleware::Error::Reqwest(e) => e.into(),
            reqwest_middleware::Error::Middleware(_) => AccountClientError::Unknown,
        }
    }
}

impl From<reqwest::Error> for AccountClientError {
    fn from(value: reqwest::Error) -> Self {
        match value.status() {
            Some(status) if status == reqwest::StatusCode::UNAUTHORIZED => {
                AccountClientError::Unauthorized
            }
            _ => AccountClientError::Unknown,
        }
    }
}

pub type AccountClientResult<T> = Result<T, AccountClientError>;

pub struct AccountClient {
    session: Session,
    client: ClientWithMiddleware,
    url: String,
    session_id: String,
}

impl AccountClient {
    fn info_header(&self) -> String {
        let info_header = HttpRequestInfoHeader::from(self.session_id.clone(), request_id());
        serde_json::to_string(&info_header).expect("Could not serialize info-header")
    }
}

impl AccountClient {
    pub fn new(session: Session, client: ClientWithMiddleware, url: String) -> Self {
        AccountClient {
            session,
            client,
            url,
            session_id: Uuid::new_v4().to_string(),
        }
    }

    pub async fn accounts(&self) -> AccountClientResult<AccountsBalancesResponse> {
        let url = format!("{}/banking/clients/user/v2/accounts/balances", self.url);
        let response = self
            .client
            .get(url)
            .header(
                AUTHORIZATION,
                format!("Bearer {}", self.session.access_token.clone()),
            )
            .header("x-http-request-info", self.info_header())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .send()
            .await?;
        match response.status() {
            reqwest::StatusCode::OK => {
                let accounts_balances: AccountsBalancesResponse = response.json().await?;
                Ok(accounts_balances)
            }
            reqwest::StatusCode::UNAUTHORIZED => Err(AccountClientError::Unauthorized),
            _ => {
                println!("Error: {:?}", response.status());
                Err(AccountClientError::Unknown)
            }
        }
    }

    pub async fn get_account_transactions(
        &self,
        account_id: &str,
        index: u32,
    ) -> AccountClientResult<TransactionsResponse> {
        let url = format!(
            "{}/banking/v1/accounts/{}/transactions?transactionState=BOOKED&paging-first={}",
            self.url, account_id, index
        );

        let response = self
            .client
            .get(&url)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .header(
                AUTHORIZATION,
                format!("Bearer {}", self.session.access_token),
            )
            .header("x-http-request-info", self.info_header())
            .send()
            .await?;

        let status = response.status();
        if status == StatusCode::OK {
            response
                .json::<TransactionsResponse>()
                .await
                .map_err(|e| {
                    eprintln!(
                        "get_account_transactions[{} idx={}]: parse failed: {e}",
                        account_id, index
                    );
                    AccountClientError::Unknown
                })
        } else {
            let body = response.text().await.unwrap_or_default();
            eprintln!(
                "get_account_transactions[{} idx={}]: {} → {}",
                account_id, index, status, body
            );
            Err(AccountClientError::Unknown)
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AccountBalances {
    paging: Paging,
    values: Vec<AccountBalance>,
}

#[derive(Debug, Deserialize)]
pub struct Paging {
    index: u32,
    matches: u32,
}

#[derive(Debug, Deserialize)]
pub struct AccountBalance {
    account_id: String,
}
