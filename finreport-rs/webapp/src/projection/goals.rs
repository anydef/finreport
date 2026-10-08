//! Projects [`crate::kafka::goals::GoalRecord`] onto `goal` (iteration 4
//! §2.2), revision-guarded last-writer-wins like every other human-decision
//! topic; a tombstone deletes the row.
//!
//! The offset commit is not this function's concern: `projection::process_batch`
//! calls it inside the batch transaction, so the row and the offset commit
//! (or roll back) together, exactly like the labeling projections.

use entity::entities::goal;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{ActiveValue::Set, ConnectionTrait, DbErr, EntityTrait};
use uuid::Uuid;

use crate::kafka::goals::{Cadence, Combine, GoalRecord, GoalType, PeriodKind};

fn goal_type_str(t: GoalType) -> &'static str {
    match t {
        GoalType::SpendingLimit => "spending_limit",
        GoalType::SavingTarget => "saving_target",
    }
}

fn combine_str(c: Combine) -> &'static str {
    match c {
        Combine::All => "all",
        Combine::Any => "any",
    }
}

fn period_kind_str(k: PeriodKind) -> &'static str {
    match k {
        PeriodKind::Recurring => "recurring",
        PeriodKind::Fixed => "fixed",
    }
}

fn cadence_str(c: Cadence) -> &'static str {
    match c {
        Cadence::Monthly => "monthly",
        Cadence::Quarterly => "quarterly",
        Cadence::Yearly => "yearly",
    }
}

/// Upserts (`Some`) or deletes (`None`) the `goal` row for `goal_id`.
///
/// The upsert only lands when the incoming `revision` is at least the stored
/// one (§2.1 last-writer-wins); an older record echoing back out of order is
/// a silent no-op.
pub async fn project_goal(
    txn: &impl ConnectionTrait,
    goal_id: Uuid,
    record: Option<GoalRecord>,
) -> Result<(), DbErr> {
    let Some(record) = record else {
        goal::Entity::delete_by_id(goal_id).exec(txn).await?;
        return Ok(());
    };

    let model = goal::ActiveModel {
        id: Set(goal_id),
        owner_user_id: Set(record.owner_user_id),
        name: Set(record.name.clone()),
        goal_type: Set(goal_type_str(record.goal_type).to_string()),
        amount: Set(record.amount),
        currency: Set(record.currency.clone()),
        scope_category_slugs: Set(record.scope.category_slugs.clone()),
        scope_tags: Set(record.scope.tags.clone()),
        scope_combine: Set(combine_str(record.scope.combine).to_string()),
        scope_tag_combine: Set(combine_str(record.scope.tag_combine).to_string()),
        period_kind: Set(period_kind_str(record.period.kind).to_string()),
        period_cadence: Set(record.period.cadence.map(|c| cadence_str(c).to_string())),
        period_start: Set(record.period.start_date),
        period_end: Set(record.period.end_date),
        archived: Set(record.archived),
        revision: Set(record.revision.into()),
    };

    let mut on_conflict = OnConflict::column(goal::Column::Id);
    on_conflict
        .update_columns([
            goal::Column::OwnerUserId,
            goal::Column::Name,
            goal::Column::GoalType,
            goal::Column::Amount,
            goal::Column::Currency,
            goal::Column::ScopeCategorySlugs,
            goal::Column::ScopeTags,
            goal::Column::ScopeCombine,
            goal::Column::ScopeTagCombine,
            goal::Column::PeriodKind,
            goal::Column::PeriodCadence,
            goal::Column::PeriodStart,
            goal::Column::PeriodEnd,
            goal::Column::Archived,
            goal::Column::Revision,
        ])
        .action_cond_where(
            Expr::col((goal::Entity, goal::Column::Revision)).lte(Expr::cust("excluded.revision")),
        );

    match goal::Entity::insert(model)
        .on_conflict(on_conflict.to_owned())
        .exec(txn)
        .await
    {
        Ok(_) => Ok(()),
        // The conditional WHERE declined a stale record (see `project_rule`).
        Err(DbErr::RecordNotInserted) => Ok(()),
        Err(e) => Err(e),
    }
}
