use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub(crate) struct AccountType {
    text: String,
}

#[derive(Deserialize, Debug)]
pub struct Account {
    iban: String,
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "accountDisplayId")]
    pub display_id: String,
    // currency: String,
    // #[serde(rename = "accountType")]
    // account_type: String,
}

#[derive(Deserialize, Debug)]
pub(crate) struct Balance {
    value: String,
    unit: String,
}

#[derive(Deserialize, Debug)]
pub struct AccountBalance {
    pub account: Account,
    #[serde(rename = "accountId")]
    pub account_id: String,

    balance: Balance,
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
