//! Internal-transfer and recurring-cost detection (iteration 3 §3): a
//! post-batch pass inside the existing `labeler` process, not a new
//! service. Both algorithms are pure functions over plain structs — no DB
//! or Kafka types in their signatures — so they unit-test directly.
//!
//! WP0 freezes the signatures below as stubs (empty results); **WP-A owns
//! the real bodies** in [`transfer`] and [`recurring`], plus the
//! `processor` loader/publisher glue.

pub mod processor;
pub mod recurring;
pub mod transfer;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use uuid::Uuid;

pub use recurring::{detect_recurring, RecurringConfig, RecurringSeries};
pub use transfer::{detect_transfers, TransferConfig, TransferPair};

/// One transaction's detection-relevant facts (iteration 3 §3), loaded by
/// [`processor`] with a single query joining `transaction`, `account`,
/// `user_account`. Deliberately DB-type-free so [`detect_transfers`] and
/// [`detect_recurring`] stay pure and unit-testable without a database.
#[derive(Debug, Clone, PartialEq)]
pub struct TxnFacts {
    pub id: Uuid,
    pub account_id: Uuid,
    /// Every user who owns `account_id` (via `user_account`); a transfer
    /// candidate pair must share at least one entry (§3.1 rule 2).
    pub owner_user_ids: Vec<Uuid>,
    pub booking_date: NaiveDate,
    pub amount: Decimal,
    pub counterparty_iban: Option<String>,
    /// Iteration 2's normalized counterparty key; `None` ⇒ never recurring
    /// (§3.2).
    pub counterparty_key: Option<String>,
}
