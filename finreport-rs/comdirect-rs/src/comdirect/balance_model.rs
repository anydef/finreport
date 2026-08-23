use crate::comdirect::raw::Raw;
use serde::Deserialize;
use serde_json::value::RawValue;

#[derive(Deserialize, Debug, Clone)]
pub struct AccountType {
    pub text: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Account {
    pub iban: String,
    pub bic: String,
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "accountDisplayId")]
    pub display_id: String,

    #[serde(rename = "accountType")]
    pub account_type: AccountType
    // currency: String,
    // #[serde(rename = "accountType")]
    // account_type: String,
}

#[derive(Deserialize, Debug)]
pub struct Balance {
    pub value: String,
    unit: String,
}

#[derive(Deserialize, Debug)]
pub struct AccountBalance {
    pub account: Account,
    #[serde(rename = "accountId")]
    pub account_id: String,

    pub balance: Balance,
    // #[serde(rename = "availableCashAmount")]
    // available_cash_amount: Balance,
}

#[derive(Deserialize, Debug, Default)]
pub struct Paging {
    index: i32,
    pub(crate) matches: i32,
}

#[derive(Deserialize, Debug)]
pub struct AccountsBalancesResponse {
    #[serde(rename = "values")]
    pub accounts: Vec<AccountBalance>,
    pub paging: Paging
}

/// Mirrors `AccountsBalancesResponse`, but each element keeps the exact raw
/// JSON it came from alongside the parsed `AccountBalance` — the Kafka path
/// needs the untouched bytes (account + balance object), the importer still
/// wants the typed fields.
#[derive(Deserialize)]
struct AccountsBalancesResponseShadow {
    #[serde(rename = "values")]
    accounts: Vec<Box<RawValue>>,
    paging: Paging,
}

pub struct AccountsBalancesResponseRaw {
    pub accounts: Vec<Raw<AccountBalance>>,
    pub paging: Paging,
}

impl AccountsBalancesResponseRaw {
    /// Parses straight from the raw response body. Each element of `values`
    /// is captured as `RawValue` first and `AccountBalance` is then parsed
    /// from that same captured slice — never from a re-serialization of an
    /// already-parsed struct — so the bytes handed to Kafka are exactly what
    /// Comdirect sent.
    pub fn from_json(body: &str) -> serde_json::Result<Self> {
        let shadow: AccountsBalancesResponseShadow = serde_json::from_str(body)?;
        let accounts = shadow
            .accounts
            .into_iter()
            .map(Raw::from_raw_value)
            .collect::<serde_json::Result<Vec<_>>>()?;
        Ok(AccountsBalancesResponseRaw {
            accounts,
            paging: shadow.paging,
        })
    }
}
