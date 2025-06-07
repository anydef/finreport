use crate::comdirect::account_client::{AccountClient, AccountClientResult};
use crate::comdirect::balance_model::{Account, AccountsBalancesResponse};
use crate::comdirect::session_client::Session;
use crate::comdirect::transaction::Transaction;
use utils::settings::Settings;

pub async fn get_accounts(
    session: Session,
    client_settings: Settings,
) -> AccountClientResult<AccountsBalancesResponse> {
    let Settings { url, .. } = client_settings;

    let account_client = AccountClient::new(session, reqwest::Client::new(), url);

    let balances_response = account_client.accounts().await?;
    Ok(balances_response)
}

pub async fn get_account_transactions(
    session: Session,
    client_settings: Settings,
    account: &Account,
) -> AccountClientResult<Vec<Transaction>> {
    let Settings { url, .. } = client_settings;

    let account_client = AccountClient::new(session, reqwest::Client::new(), url);
    let mut all_transactions: Vec<Transaction> = vec![];

    let account_id = &account.account_id;
    let transactions_response = account_client
        .get_account_transactions(&account_id, 0)
        .await?;
    let values = transactions_response.values;
    let total_transactions = transactions_response.paging.matches;
    let page_size = values.len();

    all_transactions.extend(values);

    for index in (page_size as i32..total_transactions).step_by(page_size) {
        let response = account_client
            .get_account_transactions(&account_id, index as u32)
            .await?;
        all_transactions.extend(response.values);

        println!("[{}] Fetching transactions {}/{}", account_id, index, total_transactions);
    }
    Ok(all_transactions)
}
