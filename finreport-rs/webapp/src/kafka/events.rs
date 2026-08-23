//! Carving the Comdirect payloads into per-topic records.
//!
//! The balances endpoint returns one element per account holding both the
//! account entity and its balance, but those belong on different topics
//! (`finreport.account` is compacted state, `finreport.account-balance` is an
//! event stream). Splitting them must not re-encode anything: the published
//! value has to be the bank's own bytes, so the sub-objects are pulled out as
//! borrowed `RawValue` slices of the original response rather than parsed and
//! serialized again.

use serde::Deserialize;
use serde_json::value::RawValue;

#[derive(Deserialize)]
struct AccountElement<'a> {
    #[serde(borrow)]
    account: &'a RawValue,
    #[serde(borrow)]
    balance: &'a RawValue,
}

/// Returns `(account, balance)` as slices of `element`, byte for byte.
pub fn split_account_element(element: &RawValue) -> serde_json::Result<(&RawValue, &RawValue)> {
    let parsed: AccountElement = serde_json::from_str(element.get())?;
    Ok((parsed.account, parsed.balance))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn splitting_preserves_the_original_bytes() {
        // Deliberately odd spacing and a field we do not model: whatever the
        // bank sent must survive the split untouched.
        let raw = r#"{ "accountId": "A-1",
            "account": {"accountId":"A-1",  "iban":"DE00","unmodelled":{"deep":[1,2]}},
            "balance": {"value":"12.34",   "unit":"EUR"} }"#;
        let element = RawValue::from_string(raw.to_string()).unwrap();

        let (account, balance) = split_account_element(&element).unwrap();

        assert_eq!(
            account.get(),
            r#"{"accountId":"A-1",  "iban":"DE00","unmodelled":{"deep":[1,2]}}"#
        );
        assert_eq!(balance.get(), r#"{"value":"12.34",   "unit":"EUR"}"#);
    }

    #[test]
    fn an_element_missing_balance_is_an_error_not_a_panic() {
        let element = RawValue::from_string(r#"{"account":{"a":1}}"#.to_string()).unwrap();
        assert!(split_account_element(&element).is_err());
    }
}
