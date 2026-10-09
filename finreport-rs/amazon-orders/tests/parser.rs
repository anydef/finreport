//! Parser behaviour against fixtures reconstructed from a real export. The
//! recipient columns in the fixtures hold made-up values, so no real address is
//! committed.

use amazon_orders::{Order, ParseError, PriceBasis, merge_orders, parse_orders};
use rust_decimal::Decimal;
use std::str::FromStr;

const ORDERS: &str = include_str!("fixtures/orders.csv");
const REEXPORT: &str = include_str!("fixtures/orders_reexport.csv");

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn orders() -> Vec<Order> {
    parse_orders(ORDERS.as_bytes()).expect("fixture parses")
}

fn order(id: &str) -> Order {
    orders()
        .into_iter()
        .find(|o| o.order_id == id)
        .unwrap_or_else(|| panic!("order {id} in fixture"))
}

#[test]
fn groups_rows_into_orders() {
    let all = orders();
    assert_eq!(all.len(), 13);
    assert_eq!(order("304-9357930-8158714").items.len(), 2);
    assert_eq!(order("306-0000001-0000001").items.len(), 2);
}

#[test]
fn single_item_order() {
    let o = order("304-6374005-4280367");
    assert_eq!(o.total, d("5.98"));
    assert_eq!(o.currency, "EUR");
    assert_eq!(o.order_date.to_string(), "2026-10-08");
    assert_eq!(o.items.len(), 1);
    assert_eq!(o.items[0].asin, "B0C1Z84ZQ1");
    assert_eq!(o.items[0].status, "Arriving today");
    assert!(!o.items[0].returned);
    assert_eq!(o.remainder, Decimal::ZERO);
}

#[test]
fn real_rows_cannot_settle_price_semantics() {
    // Every real sample row has quantity 1: unit price and line total coincide.
    for id in ["304-6374005-4280367", "303-9169011-8123555", "304-9906711-7622713"] {
        let o = order(id);
        assert_eq!(o.price_basis, PriceBasis::Indistinguishable, "{id}");
        assert_eq!(o.remainder, Decimal::ZERO, "{id}");
    }
}

#[test]
fn multi_item_order_with_quoted_commas_and_non_ascii() {
    let o = order("306-0000001-0000001");
    assert_eq!(o.items[0].title, "Kabel, 2 m, schwarz – Größe M (Ü-Ei)");
    assert_eq!(o.items[0].line_total + o.items[1].line_total, d("30.00"));
    assert!(o.is_reconciled());
}

/// (order id, expected basis, expected remainder, expected line totals)
#[test]
fn price_basis_is_resolved_per_order() {
    let cases: [(&str, PriceBasis, &str, &[&str]); 4] = [
        // price 6.8 x 2 + 8.99 = 22.59: unit price
        ("304-9357930-8158714", PriceBasis::UnitPrice, "0", &["13.6", "8.99"]),
        // qty 3, price 15.00, total 15.00: price is already the line total
        ("306-0000006-0000006", PriceBasis::LineTotal, "0", &["15.00"]),
        // 15.00 of a 20.00 order: neither reading reconciles, remainder explicit
        ("306-0000005-0000005", PriceBasis::Unreconciled, "5.00", &["15.00"]),
        // 10.00 with a 1 discount in a 9.00 order
        ("306-0000003-0000003", PriceBasis::Indistinguishable, "0", &["9.00"]),
    ];
    for (id, basis, remainder, lines) in cases {
        let o = order(id);
        assert_eq!(o.price_basis, basis, "{id}");
        assert_eq!(o.remainder, d(remainder), "{id}");
        let got: Vec<Decimal> = o.items.iter().map(|i| i.line_total).collect();
        let want: Vec<Decimal> = lines.iter().map(|l| d(l)).collect();
        assert_eq!(got, want, "{id}");
    }
}

#[test]
fn promotion_and_discount_are_parsed() {
    let o = order("306-0000003-0000003");
    assert!(o.discounts_netted);
    let item = &o.items[0];
    assert_eq!(item.discount, Some(d("1")));
    assert_eq!(item.promotion.as_deref(), Some("Additional discount: €1"));
    assert_eq!(item.promotion_amount, Some(d("1")));
}

#[test]
fn unfamiliar_promotion_does_not_fail_the_order() {
    let csv = ORDERS
        .replace("Additional discount: €1", "Rabatt auf Ihre Bestellung (Aktion 7)")
        .replace(",1,Rabatt", ",oops,Rabatt");
    let parsed = parse_orders(csv.as_bytes()).expect("still parses");
    let item = &parsed
        .iter()
        .find(|o| o.order_id == "306-0000003-0000003")
        .unwrap()
        .items[0];
    assert_eq!(item.discount, None);
    assert_eq!(item.promotion_amount, Some(d("7")));
}

#[test]
fn returned_item_is_identified_without_english() {
    assert!(order("306-0000002-0000002").items[0].returned);
    assert!(!order("304-6374005-4280367").items[0].returned);
    let german = ORDERS.replace("Return started", "Rückgabe eingeleitet");
    let parsed = parse_orders(german.as_bytes()).unwrap();
    let o = parsed.iter().find(|o| o.order_id == "306-0000002-0000002").unwrap();
    assert!(o.items[0].returned);
}

#[test]
fn bom_crlf_and_trailing_blank_lines() {
    let messy = format!("\u{feff}{}\n\n", ORDERS.replace('\n', "\r\n"));
    let parsed = parse_orders(messy.as_bytes()).expect("messy file parses");
    assert_eq!(parsed, orders());
}

#[test]
fn bad_header_fails_loudly_naming_the_column() {
    let swapped = ORDERS.replacen("Order Date,Total Amount", "Total Amount,Order Date", 1);
    let renamed = ORDERS.replacen("Item Price", "Unit Price", 1);
    let missing = ORDERS.replacen(",Recipient Country", "", 1);
    let extra = ORDERS.replacen("Recipient Country\n", "Recipient Country,Gift Wrap\n", 1);
    let cases = [
        (swapped, 2, "Order Date"),
        (renamed, 8, "Item Price"),
        (missing, 19, "Recipient Country"),
        (extra, 20, "<end of header>"),
    ];
    for (csv, column, name) in cases {
        let err = parse_orders(csv.as_bytes()).expect_err("must fail");
        assert!(matches!(err, ParseError::UnrecognisedHeader { .. }), "{err:?}");
        let msg = err.to_string();
        assert!(msg.contains(&format!("column {column}")), "{msg}");
        assert!(msg.contains(name), "{msg}");
    }
}

#[test]
fn headerless_file_error_leaks_nothing() {
    let without_header: String = ORDERS.lines().skip(1).collect::<Vec<_>>().join("\n");
    let err = parse_orders(without_header.as_bytes()).unwrap_err();
    assert!(!err.to_string().contains("Mustermann"));
    assert!(!format!("{err:?}").contains("Mustermann"));
}

#[test]
fn bad_cells_name_column_and_line_but_not_the_value() {
    let csv = ORDERS.replacen("2026-10-08", "08/10/2026 Musterweg 12", 1);
    let err = parse_orders(csv.as_bytes()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Order Date") && msg.contains("line 2"), "{msg}");
    assert!(!msg.contains("Musterweg"), "{msg}");

    let csv = ORDERS.replacen("5.98 EUR", "5.98", 1);
    assert!(matches!(
        parse_orders(csv.as_bytes()),
        Err(ParseError::BadCell { column: "Total Amount", .. })
    ));
}

#[test]
fn rows_of_one_order_must_agree_on_total() {
    let csv = ORDERS.replacen("22.59 EUR", "22.60 EUR", 1);
    assert!(matches!(
        parse_orders(csv.as_bytes()),
        Err(ParseError::InconsistentOrder { column: "Total Amount", .. })
    ));
}

#[test]
fn parsed_output_contains_no_recipient_data() {
    let recipient_values = [
        "Erika Mustermann",
        "Erika",
        "Mustermann",
        "Musterweg",
        "Beispielstadt",
        "Testland-Bundesland",
        "99999",
        "Germany",
    ];
    let all = orders();
    let json = serde_json::to_string(&all).unwrap();
    let debug = format!("{all:?}");
    for v in recipient_values {
        assert!(!json.contains(v), "serialised output leaks {v:?}");
        assert!(!debug.contains(v), "debug output leaks {v:?}");
    }
    // The fixture really does contain them, so the assertion has teeth.
    for v in recipient_values {
        assert!(ORDERS.contains(v));
    }
}

#[test]
fn duplicate_order_across_files_is_idempotent_and_takes_the_fresher_status() {
    let first = parse_orders(ORDERS.as_bytes()).unwrap();
    let again = parse_orders(ORDERS.as_bytes()).unwrap();
    assert_eq!(merge_orders([first.clone(), again]), first);

    let second = parse_orders(REEXPORT.as_bytes()).unwrap();
    let merged = merge_orders([first.clone(), second]);
    assert_eq!(merged.len(), first.len());
    let o = merged.iter().find(|o| o.order_id == "304-6374005-4280367").unwrap();
    assert_eq!(o.items[0].status, "Delivered 9 October");
    // First-seen position is kept.
    assert_eq!(merged[0].order_id, first[0].order_id);
}
