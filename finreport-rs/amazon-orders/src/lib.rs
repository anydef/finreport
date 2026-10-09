//! Amazon order CSV parsing and order-to-transaction matching.
//!
//! A pure library: no database, no Kafka, no GraphQL. See
//! `docs/specs/iteration-6-amazon.md`.
//!
//! * [`parse_orders`] turns the browser-extension CSV into [`Order`]s and drops
//!   the recipient (home address) columns at the boundary.
//! * [`match_orders`] joins orders to bank [`Candidate`]s on the order id.

mod error;
mod matcher;
mod model;
mod parser;
mod status;

pub use error::ParseError;
pub use matcher::{
    Candidate, Charge, ChargePlan, DescriptionKind, MatchKind, MatchReport, MatchStatus,
    OrderMatch, PlannedPart, ReviewReason, classify_description, match_orders,
};
pub use model::{Item, Order, PriceBasis};
pub use parser::{merge_orders, parse_orders};
pub use status::looks_returned;
