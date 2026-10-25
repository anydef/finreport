//! Parser and identity behaviour against synthetic fixtures.
//!
//! The fixtures preserve the *structure* of a real C24 export — ragged rows,
//! quoted German amounts, empty IBAN/BIC on card rows, a quoted counterparty
//! containing a comma, duplicate identical rows, two accounts in one file —
//! with entirely invented values. Nothing from the user's real export is
//! committed.

use std::collections::HashSet;
use std::str::FromStr;

use c24_csv::{C24Row, HEADER, ParseError, merge, parse};
use rust_decimal::Decimal;

/// 28.09.2026 – 23.10.2026.
const SEP_OCT: &str = include_str!("fixtures/export_sep_oct.csv");
/// 07.10.2026 – 02.11.2026, overlapping `SEP_OCT` from 07.10 on, with the
/// shared rows listed in a different order.
const OCT_NOV: &str = include_str!("fixtures/export_oct_nov.csv");
/// 28.09.2026 – 02.11.2026: a wider range containing both.
const WIDE: &str = include_str!("fixtures/export_wide.csv");

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn rows(csv: &str) -> Vec<C24Row> {
    parse(csv.as_bytes()).expect("fixture parses")
}

fn ids(rows: &[C24Row]) -> Vec<String> {
    rows.iter().map(|r| r.external_id.clone()).collect()
}

/// Rows matching a counterparty, in the parser's canonical order.
fn by_counterparty<'a>(rows: &'a [C24Row], name: &str) -> Vec<&'a C24Row> {
    rows.iter().filter(|r| r.counterparty == name).collect()
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[test]
fn parses_every_row_of_the_fixture() {
    assert_eq!(rows(SEP_OCT).len(), 12);
    assert_eq!(rows(OCT_NOV).len(), 12);
    assert_eq!(rows(WIDE).len(), 14);
}

#[test]
fn header_is_the_fourteen_observed_columns() {
    assert_eq!(HEADER.len(), 14);
    assert_eq!(HEADER[0], "Transaktionstyp");
    assert_eq!(HEADER[13], "Bargeldabhebung");
}

#[test]
fn card_row_has_amount_dates_and_no_counterparty_bank_details() {
    let all = rows(SEP_OCT);
    let r = by_counterparty(&all, "Example SaaS")[0];
    assert_eq!(r.transaction_type, "Online-Kartenzahlung");
    assert_eq!(r.amount, d("-21.26"));
    assert_eq!(r.booking_date.to_string(), "2026-10-07");
    assert_eq!(
        r.card_used_at.map(|t| t.to_string()).as_deref(),
        Some("2026-10-06 22:28:00")
    );
    // Card payments carry no IBAN, BIC or purpose.
    assert_eq!(r.iban, None);
    assert_eq!(r.bic, None);
    assert_eq!(r.purpose, None);
    assert_eq!(r.description.as_deref(), Some("EXAMPLE SAAS"));
    assert_eq!(r.account_number, "1234567890");
    assert_eq!(r.account_name, "C24 Smartkonto");
    let cat = r.category.as_ref().expect("row is categorised");
    assert_eq!(cat.category, "Freizeit & Unterhaltung");
    assert_eq!(
        cat.subcategory.as_deref(),
        Some("Digitale Produkte & Dienste")
    );
    // 13-field row: the trailing column is absent, not empty-but-present.
    assert_eq!(r.cash_withdrawal, None);
}

#[test]
fn transfer_row_carries_iban_bic_and_purpose_and_a_positive_amount() {
    let all = rows(SEP_OCT);
    let r = by_counterparty(&all, "Given Family, Other Family")
        .into_iter()
        .find(|r| r.booking_date.to_string() == "2026-09-28")
        .expect("the September transfer");
    assert_eq!(r.transaction_type, "Echtzeit-Überweisung");
    // Credits carry no `+`.
    assert_eq!(r.amount, d("150.00"));
    // Non-card row: Karteneinsatz is empty, which is normal.
    assert_eq!(r.card_used_at, None);
    assert_eq!(r.iban.as_deref(), Some("DE00000000000000000001"));
    assert_eq!(r.bic.as_deref(), Some("BANKDEFFXXX"));
    assert_eq!(r.purpose.as_deref(), Some("Sparkonto"));
    assert_eq!(r.description, None);
}

#[test]
fn quoted_counterparty_with_a_comma_is_one_field() {
    let all = rows(SEP_OCT);
    // Two rows share this counterparty; both must have read it whole, which
    // a hand-split on commas would have shifted every later column for.
    let matched = by_counterparty(&all, "Given Family, Other Family");
    assert_eq!(matched.len(), 2);
    for r in matched {
        assert!(r.iban.as_deref().unwrap_or("").starts_with("DE00"));
    }
}

#[test]
fn thousands_separator_parses_as_one_amount() {
    let all = rows(SEP_OCT);
    let r = all
        .iter()
        .find(|r| r.purpose.as_deref() == Some("Gehalt"))
        .expect("the salary transfer");
    assert_eq!(r.amount, d("1234.56"));
    assert_eq!(r.account_number, "9876543210");
}

#[test]
fn one_file_can_carry_several_accounts() {
    let all = rows(SEP_OCT);
    let mut accounts: Vec<(&str, &str)> = all
        .iter()
        .map(|r| (r.account_number.as_str(), r.account_name.as_str()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    accounts.sort();
    assert_eq!(
        accounts,
        vec![
            ("1234567890", "C24 Smartkonto"),
            ("9876543210", "C24 Tagesgeld"),
        ]
    );
}

#[test]
fn the_fourteen_field_row_keeps_its_trailing_value() {
    let all = rows(SEP_OCT);
    let r = by_counterparty(&all, "ATM Example Platz")[0];
    assert_eq!(r.cash_withdrawal.as_deref(), Some("Ja"));
    assert_eq!(r.amount, d("-100.00"));
}

#[test]
fn an_uncategorised_row_has_no_category() {
    let all = rows(SEP_OCT);
    let r = by_counterparty(&all, "Kiosk Example")[0];
    assert_eq!(r.category, None);
    assert_eq!(r.amount, d("-3.99"));
}

#[test]
fn an_unknown_transaction_type_does_not_reject_the_file() {
    let csv = SEP_OCT.replacen("Online-Kartenzahlung", "Völlig-Neuer-Typ", 1);
    let all = parse(csv.as_bytes()).expect("unknown types are kept as strings");
    assert!(all.iter().any(|r| r.transaction_type == "Völlig-Neuer-Typ"));
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

#[test]
fn three_identical_rows_on_one_day_get_three_distinct_stable_ids() {
    let got: Vec<String> = by_counterparty(&rows(SEP_OCT), "Wayside Inn South")
        .iter()
        .map(|r| r.external_id.clone())
        .collect();
    assert_eq!(got.len(), 3);
    assert_eq!(got.iter().collect::<HashSet<_>>().len(), 3, "ids collide");

    let occurrences: Vec<u32> = by_counterparty(&rows(SEP_OCT), "Wayside Inn South")
        .iter()
        .map(|r| r.occurrence)
        .collect();
    assert_eq!(occurrences, vec![1, 2, 3]);

    // The same three ids on every parse of the same bytes.
    for _ in 0..3 {
        let again: Vec<String> = by_counterparty(&rows(SEP_OCT), "Wayside Inn South")
            .iter()
            .map(|r| r.external_id.clone())
            .collect();
        assert_eq!(again, got);
    }
}

#[test]
fn an_identical_pair_gets_two_ids_and_another_account_starts_over() {
    let all = rows(SEP_OCT);
    let rest_stop = by_counterparty(&all, "Rest Stop North");
    assert_eq!(rest_stop.len(), 3);
    // Two on the Smartkonto, one on the Tagesgeld: identical in every other
    // field, so only the account scoping keeps them apart.
    let smart: Vec<u32> = rest_stop
        .iter()
        .filter(|r| r.account_number == "1234567890")
        .map(|r| r.occurrence)
        .collect();
    let tages: Vec<u32> = rest_stop
        .iter()
        .filter(|r| r.account_number == "9876543210")
        .map(|r| r.occurrence)
        .collect();
    assert_eq!(smart, vec![1, 2]);
    assert_eq!(tages, vec![1], "the other account's ordinal restarts at 1");
    assert_eq!(ids(&all).iter().collect::<HashSet<_>>().len(), all.len());
}

#[test]
fn external_id_is_the_content_key_plus_the_ordinal() {
    for r in rows(WIDE) {
        assert_eq!(r.external_id, format!("{}-{}", r.content_key, r.occurrence));
        assert!(r.occurrence >= 1);
        // Hex SHA-256.
        assert_eq!(r.content_key.len(), 64);
        assert!(r.content_key.chars().all(|c| c.is_ascii_hexdigit()));
    }
}

#[test]
fn no_file_produces_a_duplicate_id() {
    for csv in [SEP_OCT, OCT_NOV, WIDE] {
        let all = rows(csv);
        assert_eq!(
            ids(&all).into_iter().collect::<HashSet<_>>().len(),
            all.len()
        );
    }
}

#[test]
fn row_order_does_not_affect_ids() {
    let canonical = rows(SEP_OCT);

    let mut lines: Vec<&str> = SEP_OCT.lines().collect();
    let header = lines.remove(0);
    // Every rotation is a different listing order of the same day-complete
    // file; the real export's ordering is not something we control.
    for shift in 1..lines.len() {
        let mut rotated = lines.clone();
        rotated.rotate_left(shift);
        let csv = format!("{header}\n{}\n", rotated.join("\n"));
        let reparsed = parse(csv.as_bytes()).expect("rotation parses");
        assert_eq!(
            reparsed, canonical,
            "rotation by {shift} changed the output"
        );
    }

    // And a reversal, which a rotation never produces.
    let mut reversed = lines.clone();
    reversed.reverse();
    let csv = format!("{header}\n{}\n", reversed.join("\n"));
    assert_eq!(parse(csv.as_bytes()).unwrap(), canonical);
}

#[test]
fn overlapping_exports_derive_identical_ids_for_their_shared_rows() {
    let a = rows(SEP_OCT);
    let b = rows(OCT_NOV);

    // The overlap is the booking days 07.10 - 23.10 inclusive. C24 filters by
    // whole days, so both files list all of those days' rows.
    let day = |m, d| chrono::NaiveDate::from_ymd_opt(2026, m, d).unwrap();
    let overlap = day(10, 7)..=day(10, 23);
    let shared = |rows: &[C24Row]| -> HashSet<String> {
        rows.iter()
            .filter(|r| overlap.contains(&r.booking_date))
            .map(|r| r.external_id.clone())
            .collect()
    };
    let a_shared = shared(&a);
    let b_shared = shared(&b);

    assert_eq!(a_shared.len(), 10, "the fixtures really do share 10 rows");
    assert_eq!(
        a_shared, b_shared,
        "a shared row must derive the same external_id in both exports"
    );

    // The union of the two is the 14 distinct rows, with nothing duplicated
    // and nothing lost — including all three of the identical triple.
    let union = merge([a.clone(), b.clone()]);
    assert_eq!(union.len(), 14);
    assert_eq!(
        ids(&union).into_iter().collect::<HashSet<_>>().len(),
        14,
        "merging overlapping exports produced a duplicate id"
    );
    // Merging in the other order, and merging a file with itself, both give
    // the same id set: the import is idempotent.
    let reverse = merge([b, a.clone()]);
    assert_eq!(
        ids(&union).into_iter().collect::<HashSet<_>>(),
        ids(&reverse).into_iter().collect::<HashSet<_>>()
    );
    assert_eq!(merge([a.clone(), a.clone()]).len(), a.len());
}

#[test]
fn a_wider_range_gives_the_same_ids_for_the_days_it_shares() {
    let wide = rows(WIDE);
    for narrow in [rows(SEP_OCT), rows(OCT_NOV)] {
        let days: HashSet<chrono::NaiveDate> = narrow.iter().map(|r| r.booking_date).collect();
        let narrow_ids: HashSet<String> = ids(&narrow).into_iter().collect();
        let wide_ids: HashSet<String> = wide
            .iter()
            .filter(|r| days.contains(&r.booking_date))
            .map(|r| r.external_id.clone())
            .collect();
        assert_eq!(
            narrow_ids, wide_ids,
            "the wider export re-derived different ids for shared days"
        );
    }
    // Nothing new appears from widening except the days that were absent.
    assert_eq!(wide.len(), 14);
}

#[test]
fn recategorising_a_row_does_not_change_its_id() {
    // C24's own categorisation is evidence, not identity: it may revise it.
    let csv = SEP_OCT.replacen(
        "Freizeit & Unterhaltung,Digitale Produkte & Dienste",
        "Etwas Anderes,Ganz Anders",
        1,
    );
    let before = rows(SEP_OCT);
    let after = parse(csv.as_bytes()).unwrap();
    assert_eq!(
        ids(&before).into_iter().collect::<HashSet<_>>(),
        ids(&after).into_iter().collect::<HashSet<_>>()
    );
}

#[test]
fn rewording_a_description_does_change_the_id() {
    // The documented, accepted weakness: a re-worded row re-imports as new.
    let csv = SEP_OCT.replacen("EXAMPLE SAAS", "EXAMPLE SAAS GMBH", 1);
    let before: HashSet<String> = ids(&rows(SEP_OCT)).into_iter().collect();
    let after: HashSet<String> = ids(&parse(csv.as_bytes()).unwrap()).into_iter().collect();
    assert_ne!(before, after);
    assert_eq!(before.difference(&after).count(), 1);
}

// ---------------------------------------------------------------------------
// Whole-file rejection
// ---------------------------------------------------------------------------

#[test]
fn a_ragged_thirteen_field_row_is_accepted() {
    // Every row of the real export but one has 13 fields.
    let thirteen = rows(SEP_OCT)
        .iter()
        .filter(|r| r.cash_withdrawal.is_none())
        .count();
    assert_eq!(thirteen, 11);
}

#[test]
fn a_twelve_or_fifteen_field_row_rejects_the_whole_file() {
    let lines: Vec<&str> = SEP_OCT.lines().collect();

    // One field short of the ragged form.
    let short = lines[1].rsplit_once(',').unwrap().0;
    let csv = format!("{}\n{short}\n{}\n", lines[0], lines[2..].join("\n"));
    assert!(
        matches!(
            parse(csv.as_bytes()),
            Err(ParseError::UnexpectedFieldCount { fields: 12, .. })
        ),
        "{:?}",
        parse(csv.as_bytes())
    );

    // One field too many.
    let csv = format!(
        "{}\n{},extra,extra\n{}\n",
        lines[0],
        lines[1],
        lines[2..].join("\n")
    );
    assert!(matches!(
        parse(csv.as_bytes()),
        Err(ParseError::UnexpectedFieldCount { fields: 15, .. })
    ));

    // And nothing partial comes back: the error is the whole result.
    assert!(parse(csv.as_bytes()).is_err());
}

#[test]
fn an_unexpected_header_rejects_the_whole_file() {
    // Renamed column.
    let csv = SEP_OCT.replacen("Buchungsdatum", "Valutadatum", 1);
    let err = parse(csv.as_bytes()).unwrap_err();
    assert!(
        matches!(err, ParseError::UnrecognisedHeader { .. }),
        "{err:?}"
    );
    assert!(err.to_string().contains("Buchungsdatum"), "{err}");

    // Reordered columns: the file would otherwise be read column-shifted.
    let reordered = SEP_OCT.replacen(
        "Transaktionstyp,Buchungsdatum",
        "Buchungsdatum,Transaktionstyp",
        1,
    );
    assert!(matches!(
        parse(reordered.as_bytes()),
        Err(ParseError::UnrecognisedHeader { .. })
    ));

    // Missing header entirely: the first data row is read as the header.
    let headerless: String = SEP_OCT.lines().skip(1).collect::<Vec<_>>().join("\n");
    assert!(matches!(
        parse(headerless.as_bytes()),
        Err(ParseError::UnrecognisedHeader { .. })
    ));

    // Empty input.
    assert!(matches!(
        parse(b"".as_slice()),
        Err(ParseError::UnrecognisedHeader { .. })
    ));
}

#[test]
fn a_malformed_amount_rejects_the_whole_file() {
    let csv = SEP_OCT.replacen("\"-21,26 €\"", "\"nicht lesbar\"", 1);
    assert!(matches!(
        parse(csv.as_bytes()),
        Err(ParseError::BadCell {
            column: "Betrag",
            ..
        })
    ));
}

#[test]
fn a_malformed_date_rejects_the_whole_file() {
    let csv = SEP_OCT.replacen("07.10.2026", "2026-10-07", 1);
    assert!(matches!(
        parse(csv.as_bytes()),
        Err(ParseError::BadCell {
            column: "Buchungsdatum",
            ..
        })
    ));

    let csv = SEP_OCT.replacen("06.10.2026 22:28", "06.10.2026", 1);
    assert!(matches!(
        parse(csv.as_bytes()),
        Err(ParseError::BadCell {
            column: "Karteneinsatz",
            ..
        })
    ));
}

#[test]
fn an_empty_account_number_rejects_the_whole_file() {
    let csv = SEP_OCT.replacen(",1234567890,C24 Smartkonto,", ",,C24 Smartkonto,", 1);
    assert!(matches!(
        parse(csv.as_bytes()),
        Err(ParseError::BadCell {
            column: "Kontonummer",
            ..
        })
    ));
}

// ---------------------------------------------------------------------------
// Privacy: no error, Debug or log line may echo a cell value
// ---------------------------------------------------------------------------

/// Values that appear in the fixtures and must never reach an error message.
/// Each is an amount, a counterparty, an IBAN or a payment purpose.
const SENSITIVE: [&str; 9] = [
    "Given Family",
    "Other Family",
    "DE00000000000000000001",
    "BANKDEFFXXX",
    "Sparkonto",
    "1234567890",
    "Rest Stop North",
    "1.234,56",
    "EXAMPLE SAAS",
];

fn assert_leaks_nothing(err: &ParseError, haystack_must_contain: &str) {
    let display = err.to_string();
    let debug = format!("{err:?}");
    for v in SENSITIVE {
        assert!(!display.contains(v), "Display leaks {v:?}: {display}");
        assert!(!debug.contains(v), "Debug leaks {v:?}: {debug}");
    }
    assert!(
        !display.contains(haystack_must_contain),
        "Display leaks the offending value {haystack_must_contain:?}: {display}"
    );
    assert!(
        !debug.contains(haystack_must_contain),
        "Debug leaks the offending value {haystack_must_contain:?}: {debug}"
    );
}

#[test]
fn a_bad_amount_error_names_the_column_and_line_but_no_value() {
    let csv = SEP_OCT.replacen("\"-21,26 €\"", "\"-21,26,26 EUR Sparkonto\"", 1);
    let err = parse(csv.as_bytes()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Betrag"), "{msg}");
    assert!(msg.contains("line "), "{msg}");
    assert_leaks_nothing(&err, "-21,26,26");
}

#[test]
fn a_bad_date_error_names_the_column_and_line_but_no_value() {
    let csv = SEP_OCT.replacen("28.09.2026,", "28/09/2026 Musterweg 12,", 1);
    let err = parse(csv.as_bytes()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Buchungsdatum"), "{msg}");
    assert_leaks_nothing(&err, "Musterweg");
}

#[test]
fn a_headerless_file_error_leaks_no_row_value() {
    let headerless: String = SEP_OCT.lines().skip(1).collect::<Vec<_>>().join("\n");
    let err = parse(headerless.as_bytes()).unwrap_err();
    // The first data row, which became the "header", holds an IBAN and a
    // counterparty; neither may appear.
    assert_leaks_nothing(&err, "Weitere Ausgaben");
}

#[test]
fn a_field_count_error_leaks_no_row_value() {
    let lines: Vec<&str> = SEP_OCT.lines().collect();
    // Shorten the transfer row, the one carrying an IBAN and a purpose.
    let transfer = lines
        .iter()
        .position(|l| l.contains("DE00000000000000000001"))
        .unwrap();
    let short = lines[transfer].rsplit_once(',').unwrap().0;
    let mut mangled = lines.clone();
    mangled[transfer] = short;
    let err = parse(mangled.join("\n").as_bytes()).unwrap_err();
    assert!(
        matches!(err, ParseError::UnexpectedFieldCount { .. }),
        "{err:?}"
    );
    assert_leaks_nothing(&err, "DE00000000000000000001");
}

#[test]
fn a_malformed_csv_error_keeps_only_class_and_position() {
    // An unterminated quote makes the reader choke on the record itself,
    // whose own message would quote it.
    let csv = SEP_OCT.replacen("\"Given Family, Other Family\"", "\"Given Family", 1);
    match parse(csv.as_bytes()) {
        Err(err) => assert_leaks_nothing(&err, "Given Family"),
        // An unterminated final quote can also simply swallow the rest of the
        // file into one field, which then fails the field-count check.
        Ok(rows) => panic!("expected a rejection, got {} rows", rows.len()),
    }
}

#[test]
fn a_parsed_row_still_carries_its_own_values() {
    // The privacy rule is about *errors*. The parsed rows are the point of
    // the crate and do hold the file's values, so the assertions above have
    // teeth rather than passing on an empty parse.
    let json = serde_json::to_string(&rows(SEP_OCT)).unwrap();
    for v in [
        "Given Family, Other Family",
        "DE00000000000000000001",
        "Sparkonto",
    ] {
        assert!(json.contains(v), "parsed output lost {v:?}");
    }
}
