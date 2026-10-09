use std::fmt;

/// Parse failure. Messages name a column and a line number, **never a cell
/// value**: cells in this file can hold the user's home address.
#[derive(Debug)]
pub enum ParseError {
    /// The header row is not the known column set. Names the first mismatch.
    UnrecognisedHeader {
        position: usize,
        expected: &'static str,
        /// False when the row is shorter than the expected header. The found
        /// text is deliberately not kept: a headerless file would put a data
        /// row (an address) into the error.
        present: bool,
    },
    /// A cell could not be read as what its column requires.
    BadCell {
        line: u64,
        column: &'static str,
        reason: &'static str,
    },
    /// Two rows of one order disagree on a field that must be order-wide.
    InconsistentOrder {
        order_id: String,
        column: &'static str,
    },
    Csv(csv::Error),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnrecognisedHeader {
                position,
                expected,
                present,
            } => write!(
                f,
                "unrecognised Amazon CSV header: column {} should be {expected:?} but is {}",
                position + 1,
                if *present { "something else" } else { "missing" }
            ),
            ParseError::BadCell {
                line,
                column,
                reason,
            } => write!(f, "line {line}: column {column:?}: {reason}"),
            ParseError::InconsistentOrder { order_id, column } => write!(
                f,
                "order {order_id}: rows disagree on {column:?}, which must be the same on every row"
            ),
            ParseError::Csv(e) => write!(f, "csv error: {e}"),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<csv::Error> for ParseError {
    fn from(e: csv::Error) -> Self {
        ParseError::Csv(e)
    }
}
