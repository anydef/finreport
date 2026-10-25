use std::fmt;

/// Parse failure for a C24 CSV export.
///
/// Messages name a **column, a line and a static reason — never a cell
/// value**. Every cell in this file is bank data: an amount, a counterparty
/// name, an IBAN, a payment purpose. Echoing one into a log line or an error
/// shown to the user would leak it, so no variant here carries a `String`
/// taken from the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The header row is not the C24 export header. Carries only static
    /// column names and the observed column count: nothing read from the
    /// file, because a headerless export would put a data row (an IBAN, a
    /// counterparty) where the header should be.
    UnrecognisedHeader {
        /// Expected columns that were absent or misspelled, as static names.
        missing: Vec<&'static str>,
        /// How many columns the header row actually had.
        columns: usize,
    },
    /// A data row had a field count the export never produces. C24 writes 14
    /// fields, or 13 when the trailing `Bargeldabhebung` value is omitted;
    /// anything else means the row is not what we think it is, and the file
    /// is rejected whole rather than guessed at.
    UnexpectedFieldCount { line: u64, fields: usize },
    /// A cell could not be read as what its column requires.
    BadCell {
        line: u64,
        column: &'static str,
        reason: &'static str,
    },
    /// The CSV itself is malformed (unterminated quote, bad UTF-8, ...).
    ///
    /// `csv::Error`'s own `Display` can quote the offending record, so it is
    /// **not** forwarded. Only its class and position survive.
    Csv {
        line: Option<u64>,
        byte: Option<u64>,
        kind: &'static str,
    },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnrecognisedHeader { missing, columns } if missing.is_empty() => write!(
                f,
                "unrecognised C24 CSV header: expected exactly 14 columns, header had {columns}"
            ),
            ParseError::UnrecognisedHeader { missing, columns } => {
                let names: Vec<String> = missing.iter().map(|n| format!("{n:?}")).collect();
                write!(
                    f,
                    "unrecognised C24 CSV header: missing, misspelled or misplaced column(s) {} (header had {columns} columns)",
                    names.join(", ")
                )
            }
            ParseError::UnexpectedFieldCount { line, fields } => write!(
                f,
                "line {line}: {fields} fields, expected 13 or 14 (the whole file is rejected)"
            ),
            ParseError::BadCell {
                line,
                column,
                reason,
            } => write!(f, "line {line}: column {column:?}: {reason}"),
            ParseError::Csv { line, byte, kind } => {
                write!(f, "malformed CSV ({kind})")?;
                if let Some(line) = line {
                    write!(f, " at line {line}")?;
                }
                if let Some(byte) = byte {
                    write!(f, " (byte {byte})")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ParseError {}

impl From<csv::Error> for ParseError {
    /// Keeps the error's **class and position only**. `csv::Error`'s message
    /// can quote the record it choked on, so neither it nor the source error
    /// is retained.
    fn from(e: csv::Error) -> Self {
        let kind = match e.kind() {
            csv::ErrorKind::Io(_) => "io",
            csv::ErrorKind::Utf8 { .. } => "invalid utf-8",
            csv::ErrorKind::UnequalLengths { .. } => "unequal record lengths",
            csv::ErrorKind::Seek => "seek",
            csv::ErrorKind::Serialize(_) => "serialize",
            csv::ErrorKind::Deserialize { .. } => "deserialize",
            _ => "unknown",
        };
        let pos = match e.kind() {
            csv::ErrorKind::Utf8 { pos, .. } => pos.as_ref(),
            csv::ErrorKind::UnequalLengths { pos, .. } => pos.as_ref(),
            csv::ErrorKind::Deserialize { pos, .. } => pos.as_ref(),
            _ => None,
        };
        ParseError::Csv {
            line: pos.map(|p| p.line()),
            byte: pos.map(|p| p.byte()),
            kind,
        }
    }
}
