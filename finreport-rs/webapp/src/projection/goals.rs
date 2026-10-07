//! Projects [`crate::kafka::goals::GoalRecord`] onto `goal` (iteration 4
//! §2.2), revision-guarded last-writer-wins like every other human-decision
//! topic; a tombstone deletes the row.
//!
//! **WP0 stub.** The signature is frozen so the projector's dispatch
//! (`projection::apply_labeling_record`) compiles and handles
//! `finreport.goal` without panicking; WP-A fills in the body in this file.

use sea_orm::{ConnectionTrait, DbErr};
use uuid::Uuid;

use crate::kafka::goals::GoalRecord;

/// Upserts (`Some`) or deletes (`None`) the `goal` row for `goal_id`.
pub async fn project_goal(
    _txn: &impl ConnectionTrait,
    _goal_id: Uuid,
    _record: Option<GoalRecord>,
) -> Result<(), DbErr> {
    Ok(())
}
