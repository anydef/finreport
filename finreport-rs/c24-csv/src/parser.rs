//! CSV -> [`C24Row`]s.
//!
//! **A bad file is rejected whole, never row by row.** A half-imported bank
//! statement is worse than a rejected one: the user cannot tell which lines
//! are missing, and the gap looks like money that never moved. Every failure
//! here aborts the whole parse.

use std::io::Read;

use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;
use tracing::debug;

use crate::error::ParseError;
use crate::identity::assign_ids;
use crate::model::{C24Category, C24Row};

/// The export header, in order, verified against a real C24 export.
///
/// Matched by position and name (trimmed, case-insensitively, BOM-stripped),
/// not looked up by name like `amazon-orders` does: C24 writes a fixed header,
/// and *data rows carry no header of their own* — a row with a shifted column
/// order would be silently misread. Requiring the exact header makes that
/// impossible.
pub const HEADER: [&str; 14] = [
    "Transaktionstyp",
    "Buchungsdatum",
    "Karteneinsatz",
    "Betrag",
    "Zahlungsempfänger",
    "IBAN",
    "BIC",
    "Verwendungszweck",
    "Beschreibung",
    "Kontonummer",
    "Kontoname",
    "Kategorie",
    "Unterkategorie",
    "Bargeldabhebung",
];

// Column indices into `HEADER`.
const TRANSAKTIONSTYP: usize = 0;
const BUCHUNGSDATUM: usize = 1;
const KARTENEINSATZ: usize = 2;
const BETRAG: usize = 3;
const ZAHLUNGSEMPFAENGER: usize = 4;
const IBAN: usize = 5;
const BIC: usize = 6;
const VERWENDUNGSZWECK: usize = 7;
const BESCHREIBUNG: usize = 8;
const KONTONUMMER: usize = 9;
const KONTONAME: usize = 10;
const KATEGORIE: usize = 11;
const UNTERKATEGORIE: usize = 12;
const BARGELDABHEBUNG: usize = 13;

/// Parse one C24 export.
///
/// Rows come back in the canonical order [`assign_ids`] imposes, each with its
/// `content_key`, `occurrence` and `external_id` filled in — so two exports
/// that overlap agree on the ids of their shared rows regardless of the order
/// the file listed them in.
///
/// Fails, rejecting the whole file, on: a header that is not [`HEADER`], a row
/// whose field count is neither 13 nor 14, an unparseable date or amount, an
/// empty `Kontonummer`, or malformed CSV.
pub fn parse<R: Read>(reader: R) -> Result<Vec<C24Row>, ParseError> {
    // `flexible(true)` because the export is genuinely ragged: data rows omit
    // the trailing `Bargeldabhebung` value. The field count is checked
    // per row instead, so a 12- or 15-field row still rejects the file.
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .trim(csv::Trim::None)
        .from_reader(reader);

    check_header(rdr.headers()?)?;

    let mut rows: Vec<C24Row> = Vec::new();
    for record in rdr.records() {
        let record = record?;
        let line = record.position().map_or(0, |p| p.line());
        if record.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        rows.push(parse_row(&record, line)?);
    }

    let rows = assign_ids(rows);
    debug!(rows = rows.len(), "parsed C24 rows");
    Ok(rows)
}

/// Merge the rows of several exports, de-duplicating on `external_id`.
///
/// This is where the identity scheme pays off: overlapping date ranges produce
/// the same `external_id` for a shared row, so the union holds it once. The
/// first occurrence wins; since the id is derived from the content, a second
/// occurrence is identical on every field that matters.
pub fn merge<I: IntoIterator<Item = Vec<C24Row>>>(files: I) -> Vec<C24Row> {
    let mut out: Vec<C24Row> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in files.into_iter().flatten() {
        if seen.insert(row.external_id.clone()) {
            out.push(row);
        }
    }
    out
}

fn normalise(h: &str) -> String {
    h.trim_start_matches('\u{feff}').trim().to_lowercase()
}

/// Rejects anything that is not exactly the known header. Carries static
/// column names and a count only — a headerless export would otherwise put a
/// real IBAN into the error message.
fn check_header(header: &csv::StringRecord) -> Result<(), ParseError> {
    let got: Vec<String> = header.iter().map(normalise).collect();
    // Columns that are absent, misspelled or not where they belong.
    let missing: Vec<&'static str> = HEADER
        .iter()
        .enumerate()
        .filter(|(i, expected)| got.get(*i) != Some(&normalise(expected)))
        .map(|(_, expected)| *expected)
        .collect();
    if missing.is_empty() && got.len() == HEADER.len() {
        return Ok(());
    }
    // `missing` can be empty here only when all 14 expected columns line up
    // and the header has extra ones beyond them; `columns` reports that.
    Err(ParseError::UnrecognisedHeader {
        missing,
        columns: header.len(),
    })
}

fn parse_row(r: &csv::StringRecord, line: u64) -> Result<C24Row, ParseError> {
    // 14 fields, or 13 when the trailing `Bargeldabhebung` value is omitted —
    // which the real export does on every data row. Anything else means the
    // row is not what we think it is.
    if r.len() != HEADER.len() && r.len() != HEADER.len() - 1 {
        return Err(ParseError::UnexpectedFieldCount {
            line,
            fields: r.len(),
        });
    }

    let cell = |i: usize| r.get(i).unwrap_or("").trim();
    let opt = |i: usize| Some(cell(i)).filter(|s| !s.is_empty()).map(String::from);
    let bad = |column: &'static str, reason: &'static str| ParseError::BadCell {
        line,
        column,
        reason,
    };

    let booking_date = NaiveDate::parse_from_str(cell(BUCHUNGSDATUM), "%d.%m.%Y")
        .map_err(|_| bad("Buchungsdatum", "not a date of the form dd.MM.yyyy"))?;

    // Empty for non-card rows (transfers), which is normal, not an error.
    let card_used_at = match cell(KARTENEINSATZ) {
        "" => None,
        s => Some(
            NaiveDateTime::parse_from_str(s, "%d.%m.%Y %H:%M").map_err(|_| {
                bad(
                    "Karteneinsatz",
                    "not a date-time of the form dd.MM.yyyy HH:mm",
                )
            })?,
        ),
    };

    let amount = parse_german_amount(cell(BETRAG))
        .ok_or_else(|| bad("Betrag", "not a German-formatted amount"))?;

    let account_number = cell(KONTONUMMER);
    if account_number.is_empty() {
        return Err(bad("Kontonummer", "empty"));
    }

    let category = opt(KATEGORIE).map(|category| C24Category {
        category,
        subcategory: opt(UNTERKATEGORIE),
    });

    Ok(C24Row {
        transaction_type: cell(TRANSAKTIONSTYP).to_string(),
        booking_date,
        card_used_at,
        amount,
        counterparty: cell(ZAHLUNGSEMPFAENGER).to_string(),
        iban: opt(IBAN),
        bic: opt(BIC),
        purpose: opt(VERWENDUNGSZWECK),
        description: opt(BESCHREIBUNG),
        account_number: account_number.to_string(),
        account_name: cell(KONTONAME).to_string(),
        category,
        cash_withdrawal: if r.len() == HEADER.len() {
            opt(BARGELDABHEBUNG)
        } else {
            None
        },
        // Filled in by `assign_ids`, which needs the whole file to scope the
        // occurrence ordinal.
        content_key: String::new(),
        occurrence: 0,
        external_id: String::new(),
    })
}

/// German amount with a currency symbol: `"-1,00 €"`, `"150,00 €"`,
/// `"1.234,56 €"`.
///
/// Comma is the decimal separator, `.` the thousands separator, `-` a debit
/// prefix, and credits carry no `+`. The thousands separator is **inferred,
/// not observed**: the sample export has no amount of 1000 or more, but
/// `"1.234,56 €"` is near-certain in real data, so `.` is dropped
/// unconditionally. A currency symbol or code is ignored — the export is
/// single-currency (EUR) and carries no currency column, so there is nothing
/// to record.
///
/// Strict on purpose, unlike `amazon-orders`' tolerant reader: this is one
/// known producer with one known format, so a cell that does not match it
/// means the file is not what we think it is, and guessing would misread an
/// amount.
fn parse_german_amount(s: &str) -> Option<Decimal> {
    let s = s.trim();
    let negative = s.starts_with('-');
    let digits: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',')
        // Thousands separators dropped; see the note above.
        .collect();
    if !digits.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    if digits.matches(',').count() > 1 {
        return None;
    }
    // Reject a sign anywhere but the front ("1-00").
    if s.rfind('-').is_some_and(|i| i != 0) {
        return None;
    }
    let normalised = format!(
        "{}{}",
        if negative { "-" } else { "" },
        digits.replace(',', ".")
    );
    normalised.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn german_amounts() {
        let cases = [
            ("-1,00 €", "-1.00"),
            ("150,00 €", "150.00"),
            ("-21,26 €", "-21.26"),
            // Thousands separator: inferred, not observed in the sample.
            ("1.234,56 €", "1234.56"),
            ("-1.234.567,89 €", "-1234567.89"),
            // No decimals at all, should C24 ever write one.
            ("7 €", "7"),
            ("-7 €", "-7"),
            ("0,00 €", "0.00"),
            // No currency symbol.
            ("4,20", "4.20"),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_german_amount(input), Some(d(expected)), "{input:?}");
        }
    }

    #[test]
    fn rejects_non_amounts() {
        for input in ["", "€", "abc", "1,2,3 €", "1-00 €", "   "] {
            assert_eq!(parse_german_amount(input), None, "{input:?}");
        }
    }

    #[test]
    fn amount_scale_is_preserved_but_value_is_exact() {
        // Exactness matters: a float would round "-21,26" and the sums that
        // reconcile a statement would drift.
        assert_eq!(parse_german_amount("-21,26 €").unwrap(), d("-21.26"));
        assert!(parse_german_amount("0,10 €").unwrap() * d("3") == d("0.30"));
    }
}
