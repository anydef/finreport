//! Projects [`crate::kafka::insights::InsightRecord`] onto
//! `transaction_insight` (iteration 3 §2.3). Frozen by WP0 as a stub so
//! [`super::apply_labeling_record`] compiles and dispatches; **WP-A owns
//! the real upsert/delete body**, matching the revision-guarded,
//! idempotent shape of [`super::labeling::project_user_label`].
//!
//! The tag projection (§2.3: "replaces a transaction's whole tag set in
//! one statement pair") is WP-A's as well and will likely live alongside
//! this module; WP0 does not freeze its signature since it is not on the
//! `TOPIC_TRANSACTION_INSIGHT` dispatch path.

use sea_orm::{ConnectionTrait, DbErr};
use uuid::Uuid;

use crate::kafka::insights::InsightRecord;

/// Upserts (`Some`) or deletes (`None`) the `transaction_insight` row for
/// `transaction_id` (§2.3). **Stub**: WP0 freezes the signature only —
/// always a no-op until WP-A implements the revision-guarded write.
pub async fn project_insight(
    _txn: &impl ConnectionTrait,
    _transaction_id: Uuid,
    _record: Option<InsightRecord>,
) -> Result<(), DbErr> {
    Ok(())
}
