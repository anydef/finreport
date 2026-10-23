//! The `finreport.purchase-order` topic: retailer orders (today the Amazon
//! CSV) kept as **evidence about** bank transactions. An order is never
//! published to `finreport.transaction`; that would double-count the purchase
//! the bank line already records (iteration 6 §2.1).
//!
//! # Key
//!
//! `<user_id>:<source>:<order_id>` ([`purchase_order_topic_key`]), not the
//! spec's bare `<source>:<order_id>`:
//!
//! * an order id is unique only within a retailer, so `source` is part of it
//!   (the topic is deliberately retailer-neutral);
//! * it is **not** unique across users - two household members can each
//!   upload a file holding the same order, and the orders are per-user
//!   evidence (owned like display aliases, not shared like bank
//!   transactions), so the owner is part of the key;
//! * it is the *order*, not a shipment or charge: one order spans several
//!   bank charges, and the matcher's `Order` type is already the unit that
//!   owns them. Keying finer would split one `Order` over several records.
//!
//! Neither `source` nor an Amazon order id contains `:`
//! (`\d{3}-\d{7}-\d{7}`), which [`parse_purchase_order_topic_key`] relies on.
//!
//! # Payload and privacy
//!
//! The payload wraps [`amazon_orders::Order`] unchanged, so what the matcher
//! consumes is what the topic carries and no translation layer is needed. That
//! type has no recipient fields (the export's name/street/city/state/zip/
//! country columns are dropped at the parser boundary); this record adds only
//! ids and a revision. Compacted and long-lived: nothing address-shaped may
//! ever be added here.

use amazon_orders::Order;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Keyed `<user_id>:<source>:<order_id>`. Compacted, partitions 1, RF 1,
/// `prevent_destroy`; a tombstone deletes the order.
pub const TOPIC_PURCHASE_ORDER: &str = "finreport.purchase-order";

/// Current payload schema version for [`PurchaseOrderRecord`].
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// The `source` of orders parsed from the Amazon CSV.
pub const SOURCE_AMAZON: &str = "amazon";

/// `<user_id>:<source>:<order_id>`.
pub fn purchase_order_topic_key(user_id: Uuid, source: &str, order_id: &str) -> String {
    format!("{user_id}:{source}:{order_id}")
}

/// Inverse of [`purchase_order_topic_key`]; `None` for anything malformed.
pub fn parse_purchase_order_topic_key(topic_key: &str) -> Option<(Uuid, String, String)> {
    let mut parts = topic_key.splitn(3, ':');
    let user = Uuid::parse_str(parts.next()?).ok()?;
    let source = parts.next().filter(|s| !s.is_empty())?;
    let order_id = parts.next().filter(|s| !s.is_empty())?;
    Some((user, source.to_string(), order_id.to_string()))
}

/// Published on [`TOPIC_PURCHASE_ORDER`] when a user's order file is ingested.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PurchaseOrderRecord {
    pub schema_version: u32,
    pub user_id: Uuid,
    pub source: String,
    /// RFC 3339; last-writer-wins, so a replayed or re-uploaded older file
    /// never regresses a newer one.
    pub revision: DateTime<Utc>,
    pub order: Order,
}

impl PurchaseOrderRecord {
    pub fn topic_key(&self) -> String {
        purchase_order_topic_key(self.user_id, &self.source, &self.order.order_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amazon_orders::{Item, PriceBasis};
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    fn sample() -> PurchaseOrderRecord {
        PurchaseOrderRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            user_id: Uuid::from_bytes([1; 16]),
            source: SOURCE_AMAZON.to_string(),
            revision: Utc::now(),
            order: Order {
                order_id: "304-9357930-8158714".to_string(),
                order_date: NaiveDate::from_ymd_opt(2026, 9, 23).unwrap(),
                total: Decimal::new(2259, 2),
                currency: "EUR".to_string(),
                items: vec![Item {
                    asin: "B000TEST01".to_string(),
                    quantity: 1,
                    price: Decimal::new(899, 2),
                    discount: None,
                    promotion: None,
                    promotion_amount: None,
                    title: "Käsemesser".to_string(),
                    status: "Delivered 7 October".to_string(),
                    returned: false,
                    line_total: Decimal::new(899, 2),
                }],
                price_basis: PriceBasis::Indistinguishable,
                discounts_netted: false,
                remainder: Decimal::new(1360, 2),
            },
        }
    }

    #[test]
    fn record_and_topic_key_round_trip() {
        let record = sample();
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(record, serde_json::from_str::<PurchaseOrderRecord>(&json).unwrap());
        assert_eq!(
            parse_purchase_order_topic_key(&record.topic_key()),
            Some((record.user_id, "amazon".to_string(), "304-9357930-8158714".to_string()))
        );
    }

    #[test]
    fn amounts_travel_as_exact_decimal_strings_not_floats() {
        let value = serde_json::to_value(sample()).unwrap();
        assert_eq!(value["order"]["total"], "22.59");
        assert_eq!(value["order"]["items"][0]["line_total"], "8.99");
    }

    #[test]
    fn malformed_keys_are_rejected() {
        let user = Uuid::from_bytes([1; 16]);
        assert!(parse_purchase_order_topic_key("nope").is_none());
        assert!(parse_purchase_order_topic_key(&format!("{user}:amazon")).is_none());
        assert!(parse_purchase_order_topic_key(&format!("{user}:amazon:")).is_none());
        assert!(parse_purchase_order_topic_key(&format!("{user}::304-9357930-8158714")).is_none());
        assert!(parse_purchase_order_topic_key("x:amazon:304-9357930-8158714").is_none());
    }
}
