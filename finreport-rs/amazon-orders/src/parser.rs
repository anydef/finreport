//! CSV -> [`Order`]s.
//!
//! Columns are resolved by header name, not position. The recipient columns
//! (name, street, city, zip, country) are never looked up: no struct, log line
//! or error message in this crate can carry them.

use std::collections::HashMap;
use std::io::Read;
use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;
use rust_decimal::Decimal;
use tracing::{debug, warn};

use crate::error::ParseError;
use crate::model::{Item, Order, PriceBasis};
use crate::status::looks_returned;

/// Columns that must be present, by name (compared trimmed and
/// case-insensitively). Order is irrelevant; unknown extra columns, including
/// every recipient column, are never looked up.
const REQUIRED: [&str; 8] = [
    "Order ID",
    "Order Date",
    "Total Amount",
    "Status",
    "Item ASIN",
    "Item Quantity",
    "Item Price",
    "Item Title",
];

/// Column positions resolved from the header row.
struct Columns {
    order_id: usize,
    date: usize,
    total: usize,
    status: usize,
    asin: usize,
    qty: usize,
    price: usize,
    title: usize,
    currency: Option<usize>,
    discount: Option<usize>,
    promo: Option<usize>,
}

/// Parse one export file. Rows are grouped by `Order ID`; orders come back in
/// first-seen order.
pub fn parse_orders<R: Read>(reader: R) -> Result<Vec<Order>, ParseError> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(false)
        .trim(csv::Trim::None)
        .from_reader(reader);

    let cols = resolve_columns(rdr.headers()?)?;

    let mut order_index: HashMap<String, usize> = HashMap::new();
    let mut raw: Vec<RawOrder> = Vec::new();

    for record in rdr.records() {
        let record = record?;
        let line = record.position().map_or(0, |p| p.line());
        if record.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let row = parse_row(&record, line, &cols)?;
        match order_index.get(&row.order_id) {
            Some(&i) => {
                let o = &mut raw[i];
                if o.date != row.date {
                    return Err(ParseError::InconsistentOrder {
                        order_id: row.order_id,
                        column: "Order Date",
                    });
                }
                if o.total != row.total || o.currency != row.currency {
                    return Err(ParseError::InconsistentOrder {
                        order_id: row.order_id,
                        column: "Total Amount",
                    });
                }
                o.items.push(row.item);
            }
            None => {
                order_index.insert(row.order_id.clone(), raw.len());
                raw.push(RawOrder {
                    order_id: row.order_id,
                    date: row.date,
                    total: row.total,
                    currency: row.currency,
                    items: vec![row.item],
                });
            }
        }
    }

    let orders: Vec<Order> = raw.into_iter().map(resolve).collect();
    debug!(orders = orders.len(), "parsed Amazon orders");
    Ok(orders)
}

/// Merge orders from several files. The same order id in two files is one
/// order: the later occurrence replaces the earlier (fresher status), keeping
/// the first-seen position, so re-importing a file changes nothing.
pub fn merge_orders<I: IntoIterator<Item = Vec<Order>>>(files: I) -> Vec<Order> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Order> = Vec::new();
    for order in files.into_iter().flatten() {
        match index.get(&order.order_id) {
            Some(&i) => out[i] = order,
            None => {
                index.insert(order.order_id.clone(), out.len());
                out.push(order);
            }
        }
    }
    out
}

fn resolve_columns(header: &csv::StringRecord) -> Result<Columns, ParseError> {
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for (i, h) in header.iter().enumerate() {
        let key = h.trim_start_matches('\u{feff}').trim().to_lowercase();
        by_name.entry(key).or_insert(i);
    }
    let find = |name: &str| by_name.get(&name.to_lowercase()).copied();

    let missing: Vec<&'static str> =
        REQUIRED.iter().copied().filter(|n| find(n).is_none()).collect();
    if !missing.is_empty() {
        return Err(ParseError::UnrecognisedHeader { missing, columns: header.len() });
    }
    let req = |name: &str| find(name).expect("checked above");
    Ok(Columns {
        order_id: req("Order ID"),
        date: req("Order Date"),
        total: req("Total Amount"),
        status: req("Status"),
        asin: req("Item ASIN"),
        qty: req("Item Quantity"),
        price: req("Item Price"),
        title: req("Item Title"),
        currency: find("Currency"),
        discount: find("Item Discount"),
        promo: find("Promotions"),
    })
}

struct RawOrder {
    order_id: String,
    date: NaiveDate,
    total: Decimal,
    currency: String,
    items: Vec<Item>,
}

struct Row {
    order_id: String,
    date: NaiveDate,
    total: Decimal,
    currency: String,
    item: Item,
}

fn parse_row(r: &csv::StringRecord, line: u64, c: &Columns) -> Result<Row, ParseError> {
    let cell = |i: usize| r.get(i).unwrap_or("").trim();
    // An absent optional column reads like an empty cell.
    let opt_cell = |i: Option<usize>| i.map_or("", cell);
    let bad = |column: &'static str, reason: &'static str| ParseError::BadCell {
        line,
        column,
        reason,
    };

    let order_id = cell(c.order_id);
    if order_id.is_empty() {
        return Err(bad("Order ID", "empty"));
    }
    let date = NaiveDate::parse_from_str(cell(c.date), "%Y-%m-%d")
        .map_err(|_| bad("Order Date", "not an ISO date (YYYY-MM-DD)"))?;
    let (total, currency) = match opt_cell(c.currency).to_uppercase() {
        cur if !cur.is_empty() => {
            let total = parse_decimal(cell(c.total))
                .ok_or_else(|| bad("Total Amount", "not an amount"))?;
            (total, cur)
        }
        _ => parse_amount_with_currency(cell(c.total)).ok_or_else(|| {
            bad("Total Amount", "not of the form \"<amount> <currency>\" and no Currency column value")
        })?,
    };
    let quantity: u32 = cell(c.qty)
        .parse()
        .map_err(|_| bad("Item Quantity", "not a whole number"))?;
    let price =
        parse_decimal(cell(c.price)).ok_or_else(|| bad("Item Price", "not an amount"))?;

    let discount = match opt_cell(c.discount) {
        "" => None,
        s => {
            let parsed = parse_decimal(s);
            if parsed.is_none() {
                warn!(line, "unparseable Item Discount ignored");
            }
            parsed
        }
    };
    let promotion = Some(opt_cell(c.promo)).filter(|s| !s.is_empty()).map(String::from);
    let promotion_amount = promotion.as_deref().and_then(first_number);
    let status = cell(c.status).to_string();

    let item = Item {
        asin: cell(c.asin).to_string(),
        quantity,
        price,
        discount,
        promotion,
        promotion_amount,
        title: cell(c.title).to_string(),
        returned: looks_returned(&status),
        status,
        line_total: price,
    };
    Ok(Row {
        order_id: order_id.to_string(),
        date,
        total,
        currency,
        item,
    })
}

type Attempt<'a> = (PriceBasis, bool, &'a dyn Fn(&Item) -> Decimal);

/// Decide unit price vs line total for this order and compute the remainder.
fn resolve(mut raw: RawOrder) -> Order {
    let qty = |i: &Item| Decimal::from(i.quantity);
    let sum = |f: &dyn Fn(&Item) -> Decimal, items: &[Item]| items.iter().map(f).sum::<Decimal>();

    let as_line = |i: &Item| i.price;
    let as_unit = |i: &Item| i.price * qty(i);
    let as_line_net = |i: &Item| i.price - i.discount.unwrap_or_default();
    let as_unit_net = |i: &Item| i.price * qty(i) - i.discount.unwrap_or_default();

    let all_single = raw.items.iter().all(|i| i.quantity == 1);
    let total = raw.total;

    // (basis, netted, line function). Gross readings first, then net of
    // discount. When every quantity is 1, line == unit and the file cannot
    // tell the two apart.
    let attempts: [Attempt; 4] = [
        (
            if all_single { PriceBasis::Indistinguishable } else { PriceBasis::LineTotal },
            false,
            &as_line,
        ),
        (PriceBasis::UnitPrice, false, &as_unit),
        (
            if all_single { PriceBasis::Indistinguishable } else { PriceBasis::LineTotal },
            true,
            &as_line_net,
        ),
        (PriceBasis::UnitPrice, true, &as_unit_net),
    ];

    let (price_basis, discounts_netted, line_fn) = attempts
        .into_iter()
        .find(|(_, _, f)| sum(f, &raw.items) == total)
        .unwrap_or((PriceBasis::Unreconciled, false, &as_unit));

    for item in &mut raw.items {
        item.line_total = line_fn(item);
    }
    let remainder = total - raw.items.iter().map(|i| i.line_total).sum::<Decimal>();
    if price_basis == PriceBasis::Unreconciled {
        debug!(order_id = %raw.order_id, %remainder, "order items do not reconcile to total");
    }

    Order {
        order_id: raw.order_id,
        order_date: raw.date,
        total,
        currency: raw.currency,
        items: raw.items,
        price_basis,
        discounts_netted,
        remainder,
    }
}

static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d+(?:[.,]\d+)?").expect("static regex"));

fn first_number(s: &str) -> Option<Decimal> {
    NUMBER.find(s).and_then(|m| parse_decimal(m.as_str()))
}

/// `"5.98 EUR"`, `"EUR 5.98"`, `"5,98 €"` -> (5.98, "EUR").
fn parse_amount_with_currency(s: &str) -> Option<(Decimal, String)> {
    let currency: String = s.chars().filter(|c| c.is_alphabetic()).collect();
    let currency = if currency.is_empty() {
        if s.contains('€') {
            "EUR".to_string()
        } else {
            return None;
        }
    } else {
        currency.to_uppercase()
    };
    Some((parse_decimal(s)?, currency))
}

/// Tolerant decimal parse: ignores currency symbols/letters and whitespace,
/// accepts `1,234.56`, `1.234,56` and `5,98`.
fn parse_decimal(s: &str) -> Option<Decimal> {
    let kept: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-'))
        .collect();
    if !kept.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let normalised = match (kept.rfind('.'), kept.rfind(',')) {
        (Some(d), Some(c)) if d > c => kept.replace(',', ""),
        (Some(_), Some(_)) => kept.replace('.', "").replace(',', "."),
        (None, Some(c)) => {
            // "5,98" decimal comma; "1,234" thousands separator.
            if kept.len() - c - 1 == 3 && kept.matches(',').count() == 1 && !kept.starts_with("0,")
            {
                kept.replace(',', "")
            } else {
                kept.replace(',', ".")
            }
        }
        _ => kept,
    };
    normalised.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros_free::d;

    #[test]
    fn decimal_forms() {
        let cases = [
            ("5.98", "5.98"),
            ("6.8", "6.8"),
            ("5,98", "5.98"),
            ("1,234.56", "1234.56"),
            ("1.234,56", "1234.56"),
            ("€1", "1"),
            ("22.59 EUR", "22.59"),
            ("0", "0"),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_decimal(input), Some(d(expected)), "input {input:?}");
        }
        assert_eq!(parse_decimal("EUR"), None);
        assert_eq!(parse_decimal(""), None);
    }

    #[test]
    fn amount_with_currency() {
        assert_eq!(parse_amount_with_currency("5.98 EUR"), Some((d("5.98"), "EUR".into())));
        assert_eq!(parse_amount_with_currency("5,98 €"), Some((d("5.98"), "EUR".into())));
        assert_eq!(parse_amount_with_currency("5.98"), None);
    }
}

#[cfg(test)]
mod rust_decimal_macros_free {
    use rust_decimal::Decimal;
    use std::str::FromStr;
    pub fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }
}
