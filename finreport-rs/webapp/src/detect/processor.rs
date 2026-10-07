//! Loader/publisher glue for [`super::detect_transfers`] and
//! [`super::detect_recurring`] (iteration 3 §3): reads `TxnFacts` with one
//! query joining `transaction`, `account`, `user_account`, runs both pure
//! algorithms, and publishes `transaction_insight` records that differ from
//! the projected row (compare-before-publish, same shape as the rule
//! learner).
//!
//! WP0 freezes this entry point and wires it into
//! [`crate::labeling::processor::run`] right after the rule-learning sweep;
//! **WP-A owns the real body** — this stub loads nothing and publishes
//! nothing.

use sea_orm::{ConnectionTrait, DbErr};

use crate::kafka::producer::EventPublisher;

/// Runs one detection pass over every account (§3): internal-transfer
/// matching, then recurring-cost detection, publishing only the
/// `transaction_insight` rows that changed. **Stub**: WP0 freezes the call
/// site only — a no-op until WP-A implements the loader/publisher body.
pub async fn run_detection_pass(
    _db: &impl ConnectionTrait,
    _publisher: &EventPublisher,
) -> Result<(), DbErr> {
    Ok(())
}
