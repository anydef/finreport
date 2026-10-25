//! C24 bank CSV export parsing.
//!
//! A pure library: no database, no Kafka, no GraphQL, no projection. The same
//! shape as `amazon-orders` and for the same reason — the risky logic (German
//! amount formats, a ragged header, and deriving a stable identity for rows
//! that have no id) is testable without any infrastructure.
//!
//! * [`parse`] turns one export into [`C24Row`]s, each carrying a stable
//!   [`C24Row::external_id`].
//! * [`merge`] unions several exports, de-duplicating on that id.
//!
//! # Two binding properties
//!
//! **A bad file is rejected whole, never row by row.** A partially imported
//! bank statement is worse than a rejected one.
//!
//! **No error message or log line carries a cell value.** Every cell here is
//! bank data — an amount, a counterparty, an IBAN, a payment purpose. Errors
//! name a column, a line and a static reason only. See [`ParseError`].
//!
//! # Identity
//!
//! The export has no transaction id. Ids are derived — content hash plus an
//! occurrence ordinal scoped to the booking day, so overlapping exports agree
//! and genuinely duplicate rows stay distinct. The scheme, its reasoning and
//! its one accepted weakness are documented in the `identity` module.
//!
//! # Not decided yet
//!
//! Nothing here publishes or stores anything. The wire contract for a C24
//! record — topic, payload shape, whether `C24Row` itself becomes the
//! payload — is an open decision, so none of these types is frozen.

mod error;
mod identity;
mod model;
mod parser;

pub use error::ParseError;
pub use model::{C24Category, C24Row};
pub use parser::{HEADER, merge, parse};
