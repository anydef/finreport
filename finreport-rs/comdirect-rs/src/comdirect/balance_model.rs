use serde::Deserialize;

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
