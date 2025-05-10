use crate::comdirect::session_client::HttpRequestInfoHeader;
use crate::comdirect::session_client::Session;
use crate::comdirect::utils::request_id;
use reqwest::Client;
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

type AccountClientResult<T> = Result<T, AccountClientError>;

pub struct AccountClient {
    session: Session,
    client: Client,
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
    pub fn new(session: Session, client: Client, url: String) -> Self {
        AccountClient {
            session,
            client,
            url,
            session_id: Uuid::new_v4().to_string(),
        }
    }

    pub async fn accounts(&self) -> AccountClientResult<()> {
        let url = format!("{}/banking/clients/user/v2/accounts/balances", self.url);

        todo!("Implement the accounts method to collect all accounts' balances");
    }

    pub async fn get_account_transactions(&self) -> AccountClientResult<()> {

        todo!("Implement listing all account's transactions");
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
