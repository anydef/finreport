use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::StatusCode;
use crate::comdirect::balance::BalanceError::ResponseError;
use crate::comdirect::balance_model::AccountsBalancesResponse;
use crate::comdirect::session::{ClientSession, TokenAware};

#[derive(Debug)]
pub enum BalanceError {
    ResponseError,
}

pub async fn accounts_balances(
    session: impl TokenAware + ClientSession,
) -> Result<AccountsBalancesResponse, BalanceError> {
    let url = format!("{}/banking/clients/user/v2/accounts/balances", session.url());

    let response = session
        .client()
        .get(&url)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .header("Authorization", format!("Bearer {}", session.access_token().unwrap_or_default()))
        .header("x-http-request-info", session.info_header())

        .send().await;
    match response {
        Ok(res) => {
            if res.status() == StatusCode::OK {
                let accounts_balances: AccountsBalancesResponse = res.json().await.unwrap();
                Ok(accounts_balances)
            } else {
                Err(ResponseError)
            }
        }
        Err(e) => Err(ResponseError),
    }
}
