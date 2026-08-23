use crate::comdirect::account_client::{AccountClient, AccountClientResult};
use crate::comdirect::balance_model::{Account, AccountsBalancesResponse, AccountsBalancesResponseRaw};
use crate::comdirect::http::build_client;
use crate::comdirect::raw::Raw;
use crate::comdirect::session_client::Session;
use crate::comdirect::transaction::{filter_page_for_stop, ImportStop, PageOutcome, Transaction};
use utils::settings::ComdirectProfile;

pub async fn get_accounts(
    session: Session,
    profile: &ComdirectProfile,
) -> AccountClientResult<AccountsBalancesResponse> {
    let account_client = AccountClient::new(session, build_client(), profile.url.clone());

    let balances_response = account_client.accounts().await?;
    Ok(balances_response)
}

pub async fn get_account_transactions(
    session: Session,
    profile: &ComdirectProfile,
    account: &Account,
) -> AccountClientResult<Vec<Transaction>> {
    let account_client = AccountClient::new(session, build_client(), profile.url.clone());
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

        tracing::info!(%account_id, index, total = total_transactions, "fetching transactions");
    }
    Ok(all_transactions)
}

/// Same as `get_accounts`, but keeps each account/balance record's exact raw
/// JSON alongside its parsed form — for the Kafka path, which must publish
/// Comdirect's bytes untouched.
pub async fn get_accounts_raw(
    session: Session,
    profile: &ComdirectProfile,
) -> AccountClientResult<AccountsBalancesResponseRaw> {
    let account_client = AccountClient::new(session, build_client(), profile.url.clone());
    account_client.accounts_raw().await
}

/// Same as `get_account_transactions`, but keeps each transaction's exact raw
/// JSON alongside its parsed form. Fetches every page, same as the plain
/// version — no early-stop here, see `get_account_transactions_since` for
/// that.
pub async fn get_account_transactions_raw(
    session: Session,
    profile: &ComdirectProfile,
    account: &Account,
) -> AccountClientResult<Vec<Raw<Transaction>>> {
    let account_client = AccountClient::new(session, build_client(), profile.url.clone());
    let mut all_transactions: Vec<Raw<Transaction>> = vec![];

    let account_id = &account.account_id;
    let transactions_response = account_client
        .get_account_transactions_raw(account_id, 0)
        .await?;
    let values = transactions_response.values;
    let total_transactions = transactions_response.paging.matches;
    let page_size = values.len();

    all_transactions.extend(values);

    for index in (page_size as i32..total_transactions).step_by(page_size) {
        let response = account_client
            .get_account_transactions_raw(account_id, index as u32)
            .await?;
        all_transactions.extend(response.values);

        tracing::info!(%account_id, index, total = total_transactions, "fetching transactions (raw)");
    }
    Ok(all_transactions)
}

/// Core early-stop pagination, implemented once over `Raw<Transaction>` so
/// the Kafka dual-write path (which needs the raw bytes) and the typed-only
/// path (`get_account_transactions_since`, which just discards them) share
/// the same paging/stop/order-guard logic rather than each reimplementing it.
///
/// Only safe when Comdirect returns BOOKED transactions newest-first, which
/// isn't a documented guarantee (see the comment above
/// `transaction::filter_page_for_stop`). Each page is checked for that shape
/// before it's trusted; the first page that isn't sorted newest-first
/// disables early-stop for the rest of the account and every remaining page
/// is kept in full — this never truncates on an unordered page.
async fn get_account_transactions_since_impl(
    account_client: &AccountClient,
    account_id: &str,
    stop: &ImportStop,
) -> AccountClientResult<Vec<Raw<Transaction>>> {
    let mut all_transactions: Vec<Raw<Transaction>> = vec![];

    let mut index: u32 = 0;
    // Once true, the newest-first assumption has failed on this account:
    // early-stop is abandoned and every page from here on is kept whole.
    let mut order_violated = false;

    loop {
        let response = account_client
            .get_account_transactions_raw(account_id, index)
            .await?;
        let total_transactions = response.paging.matches;
        let values = response.values;
        let page_size = values.len();

        if page_size == 0 {
            break;
        }

        if order_violated {
            all_transactions.extend(values);
        } else {
            match filter_page_for_stop(values, stop) {
                Ok(PageOutcome::Stop(kept)) => {
                    all_transactions.extend(kept);
                    tracing::info!(%account_id, index, "early-stop reached resume point");
                    return Ok(all_transactions);
                }
                Ok(PageOutcome::Continue(kept)) => {
                    all_transactions.extend(kept);
                }
                Err(unfiltered) => {
                    tracing::warn!(
                        %account_id, idx = index,
                        "transactions page not sorted newest-first; early-stop is unsafe here, \
                         fetching all remaining pages instead of truncating"
                    );
                    order_violated = true;
                    all_transactions.extend(unfiltered);
                }
            }
        }

        tracing::info!(%account_id, index, total = total_transactions, "fetching transactions (early-stop)");

        index += page_size as u32;
        if index as i32 >= total_transactions {
            break;
        }
    }
    Ok(all_transactions)
}

/// Fetches transactions for `account`, paging only until a record at or
/// before `stop` is reached, instead of always walking every page — with
/// each returned record keeping its exact raw JSON alongside the parsed
/// form, for the Kafka dual-write path.
///
/// See `get_account_transactions_since_impl` for the early-stop/order-guard
/// semantics (identical here; this is that function without discarding the
/// raw bytes at the end).
pub async fn get_account_transactions_since_raw(
    session: Session,
    profile: &ComdirectProfile,
    account: &Account,
    stop: &ImportStop,
) -> AccountClientResult<Vec<Raw<Transaction>>> {
    let account_client = AccountClient::new(session, build_client(), profile.url.clone());
    get_account_transactions_since_impl(&account_client, &account.account_id, stop).await
}

/// Same as `get_account_transactions_since_raw`, but returns just the parsed
/// `Transaction`s — for callers that don't need the raw JSON.
pub async fn get_account_transactions_since(
    session: Session,
    profile: &ComdirectProfile,
    account: &Account,
    stop: &ImportStop,
) -> AccountClientResult<Vec<Transaction>> {
    let account_client = AccountClient::new(session, build_client(), profile.url.clone());
    let raw =
        get_account_transactions_since_impl(&account_client, &account.account_id, stop).await?;
    Ok(raw.into_iter().map(|r| r.parsed).collect())
}
