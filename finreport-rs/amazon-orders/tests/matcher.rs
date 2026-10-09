use amazon_orders::{
    Candidate, DescriptionKind, MatchKind, MatchReport, MatchStatus, Order, OrderMatch,
    ReviewReason, classify_description, match_orders, parse_orders,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::str::FromStr;

const ORDERS: &str = include_str!("fixtures/orders.csv");

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn orders() -> Vec<Order> {
    parse_orders(ORDERS.as_bytes()).unwrap()
}

fn tx(id: &str, date: &str, amount: &str, description: &str) -> Candidate {
    Candidate {
        id: id.into(),
        booking_date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
        amount: d(amount.trim()),
        description: description.into(),
    }
}

fn run(txs: &[Candidate]) -> MatchReport {
    match_orders(&orders(), txs)
}

fn find<'a>(r: &'a MatchReport, order_id: &str) -> &'a OrderMatch {
    r.matches
        .iter()
        .find(|m| m.order_id == order_id)
        .unwrap_or_else(|| panic!("no match for {order_id}: {r:#?}"))
}

fn amazon(order: &str) -> String {
    format!("01{order} AMZN Mktp DE 60...")
}

#[test]
fn extracts_ids_from_real_descriptions() {
    let cases = [
        (
            "01303-9169011-8123555 AMZN Mktp DE 60...",
            DescriptionKind::OrderId("303-9169011-8123555".into()),
        ),
        (
            "01304-9906711-7622713 AMZN Mktp DE 79...",
            DescriptionKind::OrderId("304-9906711-7622713".into()),
        ),
        (
            "304-9906711-7622713 bare id",
            DescriptionKind::OrderId("304-9906711-7622713".into()),
        ),
        ("01D01-8109513-9170236 AMZNPrime DE", DescriptionKind::Subscription),
        ("01D01-9627715-6965427 Amazon Music", DescriptionKind::Subscription),
        ("REWE SAGT DANKE 1234", DescriptionKind::NoId),
        ("AMAZON.DE", DescriptionKind::NoId),
    ];
    for (description, expected) in cases {
        assert_eq!(classify_description(description), expected, "{description}");
    }
}

#[test]
fn single_charge_applies_and_splits_by_items() {
    let r = run(&[tx("t1", "2026-10-08", "-13.4500", &amazon("303-9169011-8123555"))]);
    let m = find(&r, "303-9169011-8123555");
    assert_eq!(m.kind, MatchKind::OrderId);
    assert_eq!(m.status, MatchStatus::Apply);
    let plan = &m.split.as_ref().unwrap()[0];
    assert_eq!(plan.transaction_id, "t1");
    assert_eq!(plan.parts.len(), 1);
    assert_eq!(plan.parts[0].amount, d("13.45"));
    assert_eq!(plan.remainder, Decimal::ZERO);
}

#[test]
fn multi_item_single_charge_splits_each_item() {
    let r = run(&[tx("t1", "2026-10-02", "-30.00", &amazon("306-0000001-0000001"))]);
    let m = find(&r, "306-0000001-0000001");
    assert_eq!(m.status, MatchStatus::Apply);
    let plan = &m.split.as_ref().unwrap()[0];
    let amounts: Vec<_> = plan.parts.iter().map(|p| p.amount).collect();
    assert_eq!(amounts, vec![d("19.99"), d("10.01")]);
}

#[test]
fn two_distinct_7_99_orders_join_to_their_own_charges() {
    let r = run(&[
        tx("a", "2026-10-07", " -7.9900", &amazon("304-9906711-7622713")),
        tx("b", "2026-10-06", "-7.99", &amazon("303-5555555-6666666")),
    ]);
    for (order, txid) in [("304-9906711-7622713", "a"), ("303-5555555-6666666", "b")] {
        let m = find(&r, order);
        assert_eq!(m.status, MatchStatus::Apply, "{order}");
        assert_eq!(m.charges.len(), 1);
        assert_eq!(m.charges[0].transaction_id, txid);
    }
}

#[test]
fn multi_shipment_order_groups_both_charges_and_assigns_items() {
    let id = "304-9357930-8158714";
    let r = run(&[
        tx("c1", "2026-09-24", "-13.6000", &amazon(id)),
        tx("c2", "2026-09-23", "-8.9900", &amazon(id)),
    ]);
    let m = find(&r, id);
    assert_eq!(m.charges.len(), 2);
    assert_eq!(m.status, MatchStatus::Apply);
    let plans = m.split.as_ref().unwrap();
    let plan_for = |tid: &str| plans.iter().find(|p| p.transaction_id == tid).unwrap();
    // 13.60 is the 2 x 6.80 junction boxes (item 0); 8.99 is the cable glands.
    assert_eq!(plan_for("c1").parts.iter().map(|p| p.item_index).collect::<Vec<_>>(), vec![0]);
    assert_eq!(plan_for("c2").parts.iter().map(|p| p.item_index).collect::<Vec<_>>(), vec![1]);
}

#[test]
fn multi_shipment_with_ambiguous_items_goes_to_review_without_a_guess() {
    let id = "306-0000007-0000007"; // two 5.00 items, two 5.00 charges
    let r = run(&[
        tx("c1", "2026-09-24", "-5.00", &amazon(id)),
        tx("c2", "2026-09-25", "-5.00", &amazon(id)),
    ]);
    let m = find(&r, id);
    assert_eq!(m.split, None);
    assert_eq!(
        m.status,
        MatchStatus::NeedsReview(vec![ReviewReason::AmbiguousItemAssignment { charges: 2 }])
    );
    assert_eq!(m.charges.len(), 2, "attached to both charges for display");
}

#[test]
fn multi_shipment_with_items_that_do_not_reconcile() {
    let id = "306-0000001-0000001"; // items 19.99 + 10.01, charges 15 + 15
    let r = run(&[
        tx("c1", "2026-10-02", "-15.00", &amazon(id)),
        tx("c2", "2026-10-03", "-15.00", &amazon(id)),
    ]);
    let m = find(&r, id);
    assert_eq!(m.split, None);
    let MatchStatus::NeedsReview(reasons) = &m.status else { panic!("{:?}", m.status) };
    assert_eq!(reasons[0].to_string(), "order spans 2 charges; items do not reconcile to them");
}

#[test]
fn charges_that_do_not_sum_to_the_total_are_reported_not_split() {
    let id = "306-0000008-0000008"; // total 20.00, only 12.00 charged so far
    let r = run(&[tx("c1", "2026-09-24", "-12.00", &amazon(id))]);
    let m = find(&r, id);
    assert_eq!(m.split, None);
    assert_eq!(
        m.status,
        MatchStatus::NeedsReview(vec![ReviewReason::ChargesDoNotSumToTotal {
            charged: d("12.00"),
            total: d("20.00")
        }])
    );
}

#[test]
fn order_whose_items_do_not_reach_the_total_needs_review() {
    let id = "306-0000005-0000005"; // items 15.00 of 20.00
    let r = run(&[tx("c1", "2026-09-26", "-20.00", &amazon(id))]);
    let m = find(&r, id);
    assert_eq!(m.split, None);
    assert_eq!(
        m.status,
        MatchStatus::NeedsReview(vec![ReviewReason::OrderItemsDoNotReconcile {
            remainder: d("5.00")
        }])
    );
}

#[test]
fn returned_item_order_still_splits_and_the_refund_is_not_a_charge() {
    let id = "306-0000002-0000002";
    let r = run(&[
        tx("c1", "2026-09-29", "-12.00", &amazon(id)),
        tx("refund", "2026-10-05", "12.00", &amazon(id)),
    ]);
    let m = find(&r, id);
    assert_eq!(m.status, MatchStatus::Apply);
    assert_eq!(m.charges.len(), 1);
    assert_eq!(m.refund_transaction_ids, vec!["refund".to_string()]);
}

#[test]
fn promotion_order_splits_at_the_discounted_amount() {
    let id = "306-0000003-0000003";
    let r = run(&[tx("c1", "2026-09-28", "-9.00", &amazon(id))]);
    let m = find(&r, id);
    assert_eq!(m.status, MatchStatus::Apply);
    assert_eq!(m.split.as_ref().unwrap()[0].parts[0].amount, d("9.00"));
}

#[test]
fn order_without_a_transaction_is_unmatched() {
    let r = run(&[tx("c1", "2026-10-08", "-13.45", &amazon("303-9169011-8123555"))]);
    assert!(r.unmatched_orders.contains(&"306-0000004-0000004".to_string()));
    assert!(!r.unmatched_orders.contains(&"303-9169011-8123555".to_string()));
}

#[test]
fn transaction_without_an_order_is_unmatched() {
    let r = run(&[tx("orphan", "2026-10-08", "-9.99", &amazon("999-1234567-7654321"))]);
    assert_eq!(r.unmatched_transactions, vec!["orphan".to_string()]);
    assert!(r.matches.is_empty());
}

#[test]
fn d01_subscription_charges_are_never_unmatched() {
    let r = run(&[
        tx("prime", "2026-10-07", "-8.99", "AMAZON EU S.A R.L. 01D01-8109513-9170236 AMZNPrime DE"),
        tx("music", "2026-09-30", "-9.99", "01D01-9627715-6965427 Amazon Music"),
    ]);
    assert!(r.unmatched_transactions.is_empty());
    assert_eq!(r.subscription_transaction_ids, vec!["prime".to_string(), "music".to_string()]);
    assert!(r.matches.is_empty());
}

#[test]
fn fallback_on_amount_and_date_always_needs_confirmation() {
    let r = run(&[
        tx("nid", "2026-09-28", "-4.44", "AMAZON.DE Marketplace"),
        tx("grocer", "2026-09-28", "-4.44", "REWE 1234"),
    ]);
    let m = find(&r, "306-0000004-0000004");
    assert_eq!(m.kind, MatchKind::AmountAndDate);
    assert_eq!(m.status, MatchStatus::NeedsReview(vec![ReviewReason::AmountDateFallback]));
    assert_eq!(m.charges[0].transaction_id, "nid");
    // The non-Amazon line is ignored entirely, not reported as unmatched.
    assert!(r.unmatched_transactions.is_empty());
}

#[test]
fn fallback_outside_the_date_window_does_not_match() {
    let r = run(&[tx("late", "2026-12-01", "-4.44", "AMAZON.DE")]);
    assert!(r.matches.is_empty());
    assert_eq!(r.unmatched_transactions, vec!["late".to_string()]);
}

#[test]
fn fallback_with_two_orders_for_one_line_is_ambiguous() {
    // Both 7.99 orders, no ids on the bank line.
    let r = run(&[tx("nid", "2026-10-07", "-7.99", "AMAZON.DE")]);
    for order in ["304-9906711-7622713", "303-5555555-6666666"] {
        let m = find(&r, order);
        assert_eq!(m.split, None);
        assert!(matches!(
            &m.status,
            MatchStatus::NeedsReview(rs) if matches!(rs[0], ReviewReason::AmbiguousFallback { .. })
        ));
    }
}

#[test]
fn matching_is_idempotent_over_duplicate_orders() {
    let mut twice = orders();
    twice.extend(orders());
    let txs = [tx("t1", "2026-10-08", "-13.45", &amazon("303-9169011-8123555"))];
    let r = match_orders(&twice, &txs);
    assert_eq!(r.matches.iter().filter(|m| m.order_id == "303-9169011-8123555").count(), 1);
}
