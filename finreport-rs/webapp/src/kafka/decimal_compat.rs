//! Deserializing a `Decimal` that may be on the wire as a **number or a
//! string**, for record types that predate `serde-str`.
//!
//! # Why this exists
//!
//! `rust_decimal`'s `serde-str` feature makes every `Decimal` serialise as a
//! JSON string and, crucially, makes its `Deserialize` **reject a JSON
//! number** outright:
//!
//! ```text
//! invalid type: floating point `200.0`, expected a Decimal type
//! representing a fixed-point number
//! ```
//!
//! Cargo unifies features across the build graph, so that feature is on for
//! this crate whether or not `webapp/Cargo.toml` asks for it. The dates
//! matter:
//!
//! * **2026-10-07** — `GoalRecord::amount`, `RuleRecord::amount_min`/`_max`
//!   and `TransactionInsightRecord::recurring_median_amount` were added.
//!   Nothing in `webapp`'s graph enabled `serde-str` yet, so those records
//!   were published with **JSON numbers**.
//! * **2026-10-23** — `amazon-orders` (which does enable it) became a
//!   `webapp` dependency, and every `Decimal` in this crate silently switched
//!   to strings in both directions.
//!
//! `finreport.goal`, `.rule` and `.transaction-insight` are **compacted and
//! `prevent_destroy`**: records from that window are still on the log. A
//! projector replay that could not parse one would *skip and log it*, not
//! fail — a silent loss, and `GoalRecord::amount` is not even optional, so
//! every goal published in that window is affected.
//!
//! So these deserializers accept both forms. They are strictly more
//! permissive than the derive they replace: a payload that parsed before
//! still parses, and one that was being skipped now works. **Serialization is
//! untouched** — new records keep being written as strings, which is what the
//! record specs call for; this is only about reading what is already there.
//!
//! Apply it to a `Decimal` field on any record type that existed before
//! 2026-10-23. A record type newer than that has only ever been written as a
//! string and does not need it (`amazon-orders`' own types, the C24 records).

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer};

/// A `Decimal` as it may appear on a pre-2026-10-23 record: a JSON string
/// (current) or a JSON number (historical).
#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrNumber {
    // Tried first: the current form, and the only lossless one.
    Str(String),
    // A JSON number. `f64` is the only type serde will hand us for an
    // arbitrary JSON float, so this path inherits binary-float rounding — but
    // the alternative is dropping the record entirely, and the values
    // concerned are small money amounts that round-trip exactly at this
    // magnitude.
    Num(f64),
}

/// `#[serde(deserialize_with = "...")]` for a required `Decimal`.
pub fn de_decimal<'de, D>(deserializer: D) -> Result<Decimal, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    match StringOrNumber::deserialize(deserializer)? {
        StringOrNumber::Str(s) => s.parse().map_err(|_| {
            // No value in the message: an amount is user financial data, and
            // these records are logged on a parse failure.
            D::Error::custom("amount is not a decimal")
        }),
        StringOrNumber::Num(n) => Decimal::try_from(n)
            .map_err(|_| D::Error::custom("numeric amount is not representable as a decimal")),
    }
}

/// `#[serde(deserialize_with = "...")]` for an `Option<Decimal>`. Needs
/// `default` alongside it for the absent case, as the derive would supply.
pub fn de_decimal_opt<'de, D>(deserializer: D) -> Result<Option<Decimal>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<StringOrNumber>::deserialize(deserializer)? {
        None => Ok(None),
        Some(StringOrNumber::Str(s)) => {
            use serde::de::Error;
            s.parse().map(Some).map_err(|_| D::Error::custom("amount is not a decimal"))
        }
        Some(StringOrNumber::Num(n)) => {
            use serde::de::Error;
            Decimal::try_from(n)
                .map(Some)
                .map_err(|_| D::Error::custom("numeric amount is not representable as a decimal"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[derive(Deserialize, PartialEq, Debug)]
    struct Req {
        #[serde(deserialize_with = "de_decimal")]
        amount: Decimal,
    }

    #[derive(Deserialize, PartialEq, Debug)]
    struct Opt {
        #[serde(default, deserialize_with = "de_decimal_opt")]
        amount: Option<Decimal>,
    }

    #[test]
    fn a_string_amount_parses_exactly() {
        let got: Req = serde_json::from_str(r#"{"amount":"200.0000"}"#).unwrap();
        assert_eq!(got.amount, Decimal::from_str("200.0000").unwrap());
        // The scale is preserved, which is why the string form is the one new
        // records are written in.
        assert_eq!(got.amount.scale(), 4);
    }

    #[test]
    fn a_numeric_amount_parses_too() {
        // The historical form. Without this module the derive rejects it and
        // the whole record is skipped.
        let got: Req = serde_json::from_str(r#"{"amount":200.0}"#).unwrap();
        assert_eq!(got.amount, Decimal::from_str("200").unwrap());

        let negative: Req = serde_json::from_str(r#"{"amount":-39.9}"#).unwrap();
        assert_eq!(negative.amount, Decimal::from_str("-39.9").unwrap());

        let integral: Req = serde_json::from_str(r#"{"amount":42}"#).unwrap();
        assert_eq!(integral.amount, Decimal::from_str("42").unwrap());
    }

    #[test]
    fn the_optional_form_handles_both_plus_null_and_absent() {
        let s: Opt = serde_json::from_str(r#"{"amount":"-39.9000"}"#).unwrap();
        assert_eq!(s.amount, Some(Decimal::from_str("-39.9000").unwrap()));

        let n: Opt = serde_json::from_str(r#"{"amount":-39.9}"#).unwrap();
        assert_eq!(n.amount, Some(Decimal::from_str("-39.9").unwrap()));

        let null: Opt = serde_json::from_str(r#"{"amount":null}"#).unwrap();
        assert_eq!(null.amount, None);

        let absent: Opt = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.amount, None);
    }

    #[test]
    fn a_non_numeric_string_is_an_error_that_does_not_echo_the_value() {
        let err = serde_json::from_str::<Req>(r#"{"amount":"SECRET-AMOUNT"}"#).unwrap_err();
        let text = err.to_string();
        assert!(!text.contains("SECRET-AMOUNT"), "{text}");
    }
}
