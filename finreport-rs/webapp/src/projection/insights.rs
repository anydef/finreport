//! Projects [`crate::kafka::insights::InsightRecord`] onto
//! `transaction_insight` (iteration 3 §2.3), plus the `transaction_tag`
//! projection (§2.3 "replaces a transaction's whole tag set in one
//! statement pair") that lives alongside it.

use entity::entities::{transaction_insight, transaction_tag};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, ModelTrait, QueryFilter,
};
use uuid::Uuid;

use crate::kafka::insights::{InsightRecord, RecurringCadence, TransferMatch};
use crate::kafka::envelope::transaction_uuid;

fn transfer_match_str(m: TransferMatch) -> &'static str {
    match m {
        TransferMatch::Iban => "iban",
        TransferMatch::AmountDate => "amount_date",
    }
}

fn recurring_cadence_str(c: RecurringCadence) -> &'static str {
    match c {
        RecurringCadence::Monthly => "monthly",
        RecurringCadence::Quarterly => "quarterly",
        RecurringCadence::Yearly => "yearly",
    }
}

/// `<source>:<external_id>` (§2.2) to the deterministic transaction id, so
/// `transfer_counterpart_id` can be stored without a lookup, matching the
/// deterministic-id idiom `projection::labeling` uses (`category_uuid` etc).
fn counterpart_transaction_id(counterpart: &str) -> Option<Uuid> {
    let (source, external_id) = counterpart.split_once(':')?;
    Some(transaction_uuid(source, external_id))
}

/// Upserts (`Some`) or deletes (`None`) the `transaction_insight` row for
/// `transaction_id` (§2.3), revision-guarded like every other
/// detector/human-owned projection — a tombstone or an out-of-order replay
/// never regresses a newer row.
pub async fn project_insight(
    txn: &impl ConnectionTrait,
    transaction_id: Uuid,
    record: Option<InsightRecord>,
) -> Result<(), DbErr> {
    let Some(record) = record else {
        transaction_insight::Entity::delete_by_id(transaction_id)
            .exec(txn)
            .await?;
        return Ok(());
    };

    if let Some(existing) = transaction_insight::Entity::find_by_id(transaction_id)
        .one(txn)
        .await?
        && existing.revision > record.revision
    {
        return Ok(());
    }

    let model = transaction_insight::ActiveModel {
        transaction_id: Set(transaction_id),
        is_transfer: Set(record.is_transfer),
        transfer_counterpart_id: Set(record
            .transfer_counterpart
            .as_deref()
            .and_then(counterpart_transaction_id)),
        transfer_match: Set(record.transfer_match.map(|m| transfer_match_str(m).to_string())),
        is_recurring: Set(record.is_recurring),
        recurring_series_id: Set(record.recurring_series_id),
        recurring_cadence: Set(record.recurring_cadence.map(|c| recurring_cadence_str(c).to_string())),
        recurring_median_amount: Set(record.recurring_median_amount),
        detected_at: Set(record.detected_at.into()),
        revision: Set(record.revision.into()),
    };

    transaction_insight::Entity::insert(model)
        .on_conflict(
            sea_orm::sea_query::OnConflict::column(transaction_insight::Column::TransactionId)
                .update_columns([
                    transaction_insight::Column::IsTransfer,
                    transaction_insight::Column::TransferCounterpartId,
                    transaction_insight::Column::TransferMatch,
                    transaction_insight::Column::IsRecurring,
                    transaction_insight::Column::RecurringSeriesId,
                    transaction_insight::Column::RecurringCadence,
                    transaction_insight::Column::RecurringMedianAmount,
                    transaction_insight::Column::DetectedAt,
                    transaction_insight::Column::Revision,
                ])
                .action_cond_where(
                    Expr::col((
                        transaction_insight::Entity,
                        transaction_insight::Column::Revision,
                    ))
                    .lte(Expr::cust("excluded.revision")),
                )
                .to_owned(),
        )
        .exec(txn)
        .await?;

    Ok(())
}

/// Replaces `transaction_id`'s whole tag set in one statement pair (§2.3):
/// `DELETE` what `tags` omits, `INSERT ... ON CONFLICT DO NOTHING` for the
/// rest, guarded by `revision` like every other human-owned projection.
/// `tags` is assumed already normalized/deduped/sorted (§4) — this function
/// does not re-validate.
pub async fn project_tags(
    txn: &impl ConnectionTrait,
    transaction_id: Uuid,
    tags: &[String],
    revision: chrono::DateTime<chrono::Utc>,
) -> Result<(), DbErr> {
    let existing = transaction_tag::Entity::find()
        .filter(transaction_tag::Column::TransactionId.eq(transaction_id))
        .all(txn)
        .await?;

    let revision_tz: chrono::DateTime<chrono::FixedOffset> = revision.into();
    // Last-writer-wins (§2.1): an existing row newer than this revision
    // means a later write already landed; skip the whole replace rather
    // than let an out-of-order replay regress it.
    if existing.iter().any(|row| row.revision > revision_tz) {
        return Ok(());
    }

    for row in &existing {
        if !tags.iter().any(|t| t == &row.tag) {
            row.clone().delete(txn).await?;
        }
    }

    for tag in tags {
        let model = transaction_tag::ActiveModel {
            transaction_id: Set(transaction_id),
            tag: Set(tag.clone()),
            revision: Set(revision.into()),
        };
        transaction_tag::Entity::insert(model)
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([
                    transaction_tag::Column::TransactionId,
                    transaction_tag::Column::Tag,
                ])
                .do_nothing()
                .to_owned(),
            )
            .exec(txn)
            .await?;
    }

    Ok(())
}
