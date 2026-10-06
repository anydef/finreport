//! Custom GraphQL scalar newtypes (§5): `Date`, `DateTime`, `Decimal`, `UUID`.
//!
//! Frozen by WP0 as part of the SDL contract — `async-graphql` would happily
//! reuse its built-in `ID`/serde impls for `chrono`/`uuid`/`rust_decimal`
//! types, but that loses the type *names* the spec requires (`Date` rather
//! than a raw string, `Decimal` rather than a float, `UUID` rather than `ID`).
//! Money is never serialized as a float — `Decimal` always carries its exact
//! string form end to end.

use async_graphql::{InputValueError, InputValueResult, Scalar, ScalarType, Value};
use chrono::{DateTime as ChronoDateTime, NaiveDate, Utc};
use rust_decimal::Decimal as RustDecimal;
use std::str::FromStr;
use uuid::Uuid as RustUuid;

/// `"YYYY-MM-DD"` calendar date (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date(pub NaiveDate);

#[Scalar(name = "Date")]
impl ScalarType for Date {
    fn parse(value: Value) -> InputValueResult<Self> {
        match value {
            Value::String(s) => NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                .map(Date)
                .map_err(|e| InputValueError::custom(format!("invalid Date '{s}': {e}"))),
            _ => Err(InputValueError::expected_type(value)),
        }
    }

    fn to_value(&self) -> Value {
        Value::String(self.0.format("%Y-%m-%d").to_string())
    }
}

/// RFC 3339 instant (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime(pub ChronoDateTime<Utc>);

#[Scalar(name = "DateTime")]
impl ScalarType for DateTime {
    fn parse(value: Value) -> InputValueResult<Self> {
        match value {
            Value::String(s) => ChronoDateTime::parse_from_rfc3339(&s)
                .map(|dt| DateTime(dt.with_timezone(&Utc)))
                .map_err(|e| InputValueError::custom(format!("invalid DateTime '{s}': {e}"))),
            _ => Err(InputValueError::expected_type(value)),
        }
    }

    fn to_value(&self) -> Value {
        Value::String(self.0.to_rfc3339())
    }
}

/// Exact decimal, serialized as a string — e.g. `"-12.3400"` — never a float
/// (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decimal(pub RustDecimal);

#[Scalar(name = "Decimal")]
impl ScalarType for Decimal {
    fn parse(value: Value) -> InputValueResult<Self> {
        match value {
            Value::String(s) => RustDecimal::from_str(&s)
                .map(Decimal)
                .map_err(|e| InputValueError::custom(format!("invalid Decimal '{s}': {e}"))),
            _ => Err(InputValueError::expected_type(value)),
        }
    }

    fn to_value(&self) -> Value {
        Value::String(self.0.to_string())
    }
}

/// A `UUID` scalar distinct from async-graphql's built-in `ID`, so the SDL
/// spells it `UUID` (§5) — `ID` is reserved for the opaque Sankey node/link
/// ids, which are not necessarily `UUID`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Uuid(pub RustUuid);

#[Scalar(name = "UUID")]
impl ScalarType for Uuid {
    fn parse(value: Value) -> InputValueResult<Self> {
        match value {
            Value::String(s) => RustUuid::parse_str(&s)
                .map(Uuid)
                .map_err(|e| InputValueError::custom(format!("invalid UUID '{s}': {e}"))),
            _ => Err(InputValueError::expected_type(value)),
        }
    }

    fn to_value(&self) -> Value {
        Value::String(self.0.to_string())
    }
}

impl From<RustUuid> for Uuid {
    fn from(value: RustUuid) -> Self {
        Uuid(value)
    }
}

impl From<NaiveDate> for Date {
    fn from(value: NaiveDate) -> Self {
        Date(value)
    }
}

impl From<RustDecimal> for Decimal {
    fn from(value: RustDecimal) -> Self {
        Decimal(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_round_trips_through_its_wire_format() {
        let date = Date(NaiveDate::from_ymd_opt(2024, 6, 15).unwrap());
        let value = date.to_value();
        assert_eq!(Date::parse(value).unwrap(), date);
    }

    #[test]
    fn date_rejects_non_iso_input() {
        assert!(Date::parse(Value::String("06/15/2024".to_string())).is_err());
    }

    #[test]
    fn decimal_preserves_exact_string_form() {
        let decimal = Decimal(RustDecimal::from_str("-12.3400").unwrap());
        assert_eq!(decimal.to_value(), Value::String("-12.3400".to_string()));
    }

    #[test]
    fn uuid_round_trips_through_its_wire_format() {
        let id = Uuid(RustUuid::parse_str("1f6a6e2b-9b3d-4c8a-ab9e-5a2b0c7d4e11").unwrap());
        let value = id.to_value();
        assert_eq!(Uuid::parse(value).unwrap(), id);
    }

    #[test]
    fn datetime_round_trips_as_rfc3339() {
        let dt = DateTime(
            ChronoDateTime::parse_from_rfc3339("2024-06-15T08:30:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let value = dt.to_value();
        assert_eq!(DateTime::parse(value).unwrap(), dt);
    }
}
