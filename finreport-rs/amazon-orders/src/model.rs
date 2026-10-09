//! Parsed order types. Deliberately has **no** recipient fields: the home
//! address is dropped at the parser boundary and cannot be represented here.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// How `Item Price` relates to `Total Amount`, resolved per order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PriceBasis {
    /// Every quantity is 1, so unit price and line total are the same number.
    /// The file cannot settle the question for this order.
    Indistinguishable,
    /// `Item Price` is per unit; the line total is `price x quantity`.
    UnitPrice,
    /// `Item Price` is already the line total.
    LineTotal,
    /// Neither reading reconciles to the order total. Line totals use
    /// `price x quantity` and the difference is in [`Order::remainder`].
    Unreconciled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub asin: String,
    pub quantity: u32,
    /// `Item Price` exactly as supplied (unit price or line total, see
    /// [`Order::price_basis`]).
    pub price: Decimal,
    /// `Item Discount`, when present and parseable.
    pub discount: Option<Decimal>,
    /// `Promotions` text as supplied, when present.
    pub promotion: Option<String>,
    /// First number found in the promotion text, when any (`€1` -> 1).
    pub promotion_amount: Option<Decimal>,
    pub title: String,
    /// `Status` as supplied. Free text and localised.
    pub status: String,
    /// The status looks like a return or refund (language-tolerant).
    pub returned: bool,
    /// Resolved line amount (see [`Order::price_basis`] and
    /// [`Order::discounts_netted`]).
    pub line_total: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub order_id: String,
    pub order_date: NaiveDate,
    pub total: Decimal,
    pub currency: String,
    pub items: Vec<Item>,
    pub price_basis: PriceBasis,
    /// Item discounts were subtracted to make the items reconcile.
    pub discounts_netted: bool,
    /// `total - sum(item.line_total)`: shipping, tax, gift cards, promotions
    /// or anything else the file does not explain. Zero when reconciled.
    pub remainder: Decimal,
}

impl Order {
    pub fn is_reconciled(&self) -> bool {
        self.price_basis != PriceBasis::Unreconciled
    }
}
