//! Order -> bank transaction matching. Pure functions over given inputs.
//!
//! The order id in the bank description is an exact join key. Amount and date
//! are a fallback for lines without an id and never produce an automatic match.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::LazyLock;

use chrono::{Duration, NaiveDate};
use regex::Regex;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::model::{Order, PriceBasis};

/// A bank transaction offered to the matcher.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub booking_date: NaiveDate,
    /// Signed: debits are negative, as on the bank statement.
    pub amount: Decimal,
    pub description: String,
}

/// The leading `01`/`02`/`03` is Comdirect's remittance-field marker, not part
/// of the id. (The spec's bare `\b(\d{3}-\d{7}-\d{7})\b` cannot match
/// `01303-9169011-8123555`: there is no word boundary inside `01303`.)
static ORDER_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:0[1-9])?(\d{3}-\d{7}-\d{7})\b").expect("static regex")
});
/// Prime / Amazon Music subscription charges (`01D01-8109513-9170236`).
static SUBSCRIPTION_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:0[1-9])?D01-\d{7}-\d{7}\b").expect("static regex"));

/// How far after the order date a fallback charge may be booked.
const FALLBACK_WINDOW_DAYS: i64 = 14;
/// Item-to-charge assignment search is exponential; beyond this, review.
const MAX_ITEMS_FOR_ASSIGNMENT: usize = 12;

/// What the description says about the charge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DescriptionKind {
    OrderId(String),
    Subscription,
    NoId,
}

pub fn classify_description(description: &str) -> DescriptionKind {
    if SUBSCRIPTION_ID.is_match(description) {
        return DescriptionKind::Subscription;
    }
    match ORDER_ID.captures(description) {
        Some(c) => DescriptionKind::OrderId(c[1].to_string()),
        None => DescriptionKind::NoId,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchKind {
    /// Joined on the order id in the bank description.
    OrderId,
    /// Joined on amount and date only: always needs confirmation.
    AmountAndDate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ReviewReason {
    /// No order id in the description; amount and date matched.
    AmountDateFallback,
    /// Several orders or charges fit the amount and date equally well.
    AmbiguousFallback { candidates: usize },
    /// Charges carrying this order id do not add up to the order total.
    ChargesDoNotSumToTotal { charged: Decimal, total: Decimal },
    /// More than one charge and the item prices do not reconcile to them.
    ItemsDoNotReconcileToCharges { charges: usize },
    /// More than one way to assign the items to the charges fits equally.
    AmbiguousItemAssignment { charges: usize },
    /// Too many items to try assignments.
    TooManyItems { items: usize },
    /// The order's items do not add up to its own total.
    OrderItemsDoNotReconcile { remainder: Decimal },
}

impl fmt::Display for ReviewReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AmountDateFallback => write!(
                f,
                "no order id on the bank line; matched on amount and date only"
            ),
            Self::AmbiguousFallback { candidates } => write!(
                f,
                "no order id on the bank line and {candidates} candidates fit its amount and date"
            ),
            Self::ChargesDoNotSumToTotal { charged, total } => write!(
                f,
                "charges total {charged} but the order total is {total}; a shipment may be missing or still pending"
            ),
            Self::ItemsDoNotReconcileToCharges { charges } => write!(
                f,
                "order spans {charges} charges; items do not reconcile to them"
            ),
            Self::AmbiguousItemAssignment { charges } => write!(
                f,
                "order spans {charges} charges; more than one assignment of items to charges fits"
            ),
            Self::TooManyItems { items } => {
                write!(f, "order has {items} items, too many to assign across charges")
            }
            Self::OrderItemsDoNotReconcile { remainder } => write!(
                f,
                "item prices do not add up to the order total (difference {remainder})"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MatchStatus {
    /// Safe to apply without confirmation.
    Apply,
    /// A person must confirm; the reasons are user-facing.
    NeedsReview(Vec<ReviewReason>),
}

/// One item (by index into `Order::items`) and the amount it contributes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedPart {
    pub item_index: usize,
    pub amount: Decimal,
}

/// How one charge splits into items. `remainder` is the explicit "shipping &
/// adjustments" part: `charge - sum(parts)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChargePlan {
    pub transaction_id: String,
    /// Positive amount charged.
    pub charge: Decimal,
    pub parts: Vec<PlannedPart>,
    pub remainder: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Charge {
    pub transaction_id: String,
    /// Positive amount charged.
    pub amount: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderMatch {
    pub order_id: String,
    pub kind: MatchKind,
    /// Every debit joined to the order (several for a multi-shipment order).
    pub charges: Vec<Charge>,
    /// Credits carrying the same order id (refunds). Never split.
    pub refund_transaction_ids: Vec<String>,
    /// Item split per charge. `None` when it cannot be assigned without
    /// guessing; the order is then attached to the charges for display only.
    pub split: Option<Vec<ChargePlan>>,
    pub status: MatchStatus,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MatchReport {
    pub matches: Vec<OrderMatch>,
    /// Orders with no bank line.
    pub unmatched_orders: Vec<String>,
    /// Bank lines carrying an order id that is not in the orders given, plus
    /// Amazon-looking lines with no id that no order claimed. Subscriptions
    /// are never listed here.
    pub unmatched_transactions: Vec<String>,
    /// `D01-` Prime / Music charges: recognised and set aside.
    pub subscription_transaction_ids: Vec<String>,
}

pub fn match_orders(orders: &[Order], transactions: &[Candidate]) -> MatchReport {
    let mut report = MatchReport::default();
    let mut by_order: BTreeMap<&str, Vec<&Candidate>> = BTreeMap::new();
    let mut no_id: Vec<&Candidate> = Vec::new();

    for tx in transactions {
        match classify_description(&tx.description) {
            DescriptionKind::Subscription => {
                report.subscription_transaction_ids.push(tx.id.clone())
            }
            DescriptionKind::OrderId(id) => {
                // Key borrowed from the description-owned String below.
                let key = orders.iter().find(|o| o.order_id == id).map(|o| o.order_id.as_str());
                match key {
                    Some(k) => by_order.entry(k).or_default().push(tx),
                    None => report.unmatched_transactions.push(tx.id.clone()),
                }
            }
            DescriptionKind::NoId => {
                if looks_like_amazon(&tx.description) {
                    no_id.push(tx);
                }
            }
        }
    }

    let mut order_ids_seen = HashSet::new();
    let mut leftover: Vec<&Order> = Vec::new();
    for order in orders {
        if !order_ids_seen.insert(order.order_id.as_str()) {
            continue; // duplicate order in the input: one match only
        }
        match by_order.get(order.order_id.as_str()) {
            Some(txs) => {
                if let Some(m) = match_by_id(order, txs) {
                    report.matches.push(m);
                } else {
                    // Only credits carried the id.
                    leftover.push(order);
                }
            }
            None => leftover.push(order),
        }
    }

    fallback(&leftover, &no_id, &mut report);
    debug!(
        matched = report.matches.len(),
        unmatched_orders = report.unmatched_orders.len(),
        unmatched_transactions = report.unmatched_transactions.len(),
        "matched Amazon orders"
    );
    report
}

fn looks_like_amazon(description: &str) -> bool {
    let d = description.to_lowercase();
    d.contains("amazon") || d.contains("amzn")
}

/// Join on the order id. `None` when no debit carries it.
fn match_by_id(order: &Order, txs: &[&Candidate]) -> Option<OrderMatch> {
    let mut charges = Vec::new();
    let mut refunds = Vec::new();
    for tx in txs {
        if tx.amount.is_sign_negative() && !tx.amount.is_zero() {
            charges.push(Charge {
                transaction_id: tx.id.clone(),
                amount: -tx.amount,
            });
        } else {
            refunds.push(tx.id.clone());
        }
    }
    if charges.is_empty() {
        return None;
    }
    // Stable, input-independent order: by amount then id.
    charges.sort_by(|a, b| a.amount.cmp(&b.amount).then(a.transaction_id.cmp(&b.transaction_id)));

    let charged: Decimal = charges.iter().map(|c| c.amount).sum();
    let mut reasons = Vec::new();
    let mut split = None;

    if charged != order.total {
        reasons.push(ReviewReason::ChargesDoNotSumToTotal {
            charged,
            total: order.total,
        });
    } else if order.price_basis == PriceBasis::Unreconciled {
        reasons.push(ReviewReason::OrderItemsDoNotReconcile {
            remainder: order.remainder,
        });
    } else if charges.len() == 1 {
        split = Some(vec![plan_single(order, &charges[0])]);
    } else {
        match assign_items(order, &charges) {
            Assignment::Unique(plans) => split = Some(plans),
            Assignment::None => reasons.push(ReviewReason::ItemsDoNotReconcileToCharges {
                charges: charges.len(),
            }),
            Assignment::Ambiguous => reasons.push(ReviewReason::AmbiguousItemAssignment {
                charges: charges.len(),
            }),
            Assignment::TooMany => reasons.push(ReviewReason::TooManyItems {
                items: order.items.len(),
            }),
        }
    }

    let status = if reasons.is_empty() {
        MatchStatus::Apply
    } else {
        MatchStatus::NeedsReview(reasons)
    };
    Some(OrderMatch {
        order_id: order.order_id.clone(),
        kind: MatchKind::OrderId,
        charges,
        refund_transaction_ids: refunds,
        split,
        status,
    })
}

fn plan_single(order: &Order, charge: &Charge) -> ChargePlan {
    let parts: Vec<PlannedPart> = order
        .items
        .iter()
        .enumerate()
        .map(|(item_index, i)| PlannedPart {
            item_index,
            amount: i.line_total,
        })
        .collect();
    let remainder = charge.amount - parts.iter().map(|p| p.amount).sum::<Decimal>();
    ChargePlan {
        transaction_id: charge.transaction_id.clone(),
        charge: charge.amount,
        parts,
        remainder,
    }
}

enum Assignment {
    Unique(Vec<ChargePlan>),
    None,
    Ambiguous,
    TooMany,
}

/// Find every way to put each item (by `line_total`) on exactly one charge so
/// that each charge's items sum to exactly that charge. Only a *unique*
/// solution is accepted.
fn assign_items(order: &Order, charges: &[Charge]) -> Assignment {
    let n = order.items.len();
    if n > MAX_ITEMS_FOR_ASSIGNMENT {
        return Assignment::TooMany;
    }
    let mut solutions: Vec<Vec<usize>> = Vec::new();
    let mut current = vec![0usize; n];
    let mut sums = vec![Decimal::ZERO; charges.len()];
    search(order, charges, 0, &mut current, &mut sums, &mut solutions);

    match solutions.len() {
        0 => Assignment::None,
        1 => {
            let sol = &solutions[0];
            let plans = charges
                .iter()
                .enumerate()
                .map(|(ci, c)| ChargePlan {
                    transaction_id: c.transaction_id.clone(),
                    charge: c.amount,
                    parts: sol
                        .iter()
                        .enumerate()
                        .filter(|&(_, &assigned)| assigned == ci)
                        .map(|(item_index, _)| PlannedPart {
                            item_index,
                            amount: order.items[item_index].line_total,
                        })
                        .collect(),
                    remainder: Decimal::ZERO,
                })
                .collect();
            Assignment::Unique(plans)
        }
        _ => Assignment::Ambiguous,
    }
}

fn search(
    order: &Order,
    charges: &[Charge],
    idx: usize,
    current: &mut Vec<usize>,
    sums: &mut Vec<Decimal>,
    out: &mut Vec<Vec<usize>>,
) {
    if out.len() > 1 {
        return; // already ambiguous
    }
    if idx == order.items.len() {
        if charges.iter().zip(sums.iter()).all(|(c, s)| c.amount == *s) {
            out.push(current.clone());
        }
        return;
    }
    let amount = order.items[idx].line_total;
    for ci in 0..charges.len() {
        if sums[ci] + amount > charges[ci].amount {
            continue; // prices are positive: prune
        }
        sums[ci] += amount;
        current[idx] = ci;
        search(order, charges, idx + 1, current, sums, out);
        sums[ci] -= amount;
    }
}

/// Amount-and-date matching for lines without an id. Never `Apply`.
fn fallback(orders: &[&Order], txs: &[&Candidate], report: &mut MatchReport) {
    let window = Duration::days(FALLBACK_WINDOW_DAYS);
    let fits = |o: &Order, t: &Candidate| {
        t.amount.is_sign_negative()
            && -t.amount == o.total
            && t.booking_date >= o.order_date
            && t.booking_date <= o.order_date + window
    };

    let mut claimed: HashSet<&str> = HashSet::new();
    let mut tx_demand: HashMap<&str, usize> = HashMap::new();
    for o in orders {
        for t in txs.iter().filter(|t| fits(o, t)) {
            *tx_demand.entry(t.id.as_str()).or_default() += 1;
        }
    }

    for o in orders {
        let candidates: Vec<&&Candidate> = txs.iter().filter(|t| fits(o, t)).collect();
        match candidates.as_slice() {
            [] => report.unmatched_orders.push(o.order_id.clone()),
            [t] if tx_demand[t.id.as_str()] == 1 => {
                claimed.insert(t.id.as_str());
                let charge = Charge {
                    transaction_id: t.id.clone(),
                    amount: -t.amount,
                };
                let split = (o.price_basis != PriceBasis::Unreconciled)
                    .then(|| vec![plan_single(o, &charge)]);
                report.matches.push(OrderMatch {
                    order_id: o.order_id.clone(),
                    kind: MatchKind::AmountAndDate,
                    charges: vec![charge],
                    refund_transaction_ids: vec![],
                    split,
                    status: MatchStatus::NeedsReview(vec![ReviewReason::AmountDateFallback]),
                });
            }
            many => {
                for t in many {
                    claimed.insert(t.id.as_str());
                }
                let mut ids: Vec<&Candidate> = many.iter().map(|t| **t).collect();
                ids.sort_by(|a, b| a.id.cmp(&b.id));
                report.matches.push(OrderMatch {
                    order_id: o.order_id.clone(),
                    kind: MatchKind::AmountAndDate,
                    charges: ids
                        .iter()
                        .map(|t| Charge {
                            transaction_id: t.id.clone(),
                            amount: -t.amount,
                        })
                        .collect(),
                    refund_transaction_ids: vec![],
                    split: None,
                    status: MatchStatus::NeedsReview(vec![ReviewReason::AmbiguousFallback {
                        candidates: many.len().max(tx_demand.get(many[0].id.as_str()).copied().unwrap_or(0)),
                    }]),
                });
            }
        }
    }

    for t in txs {
        if !claimed.contains(t.id.as_str()) {
            report.unmatched_transactions.push(t.id.clone());
        }
    }
}
