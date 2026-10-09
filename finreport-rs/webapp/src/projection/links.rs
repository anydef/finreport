//! Projects [`crate::kafka::links::TransactionLinkRecord`] onto
//! `transaction_link` + `transaction_link_member`.
//!
//! Revision-guarded last-writer-wins like every other human-decision topic:
//! a record older than the stored one is a no-op, so a replay or an
//! out-of-order echo never regresses a newer edit. A tombstone deletes the
//! link and its members, which is all "removing a link" has to mean - nothing
//! else was ever changed on the linked transactions, so the previous view is
//! restored exactly.
//!
//! The members are replaced wholesale with the link row, inside the batch
//! transaction `projection::process_batch` already holds, so a reader never
//! sees a link with half its members.

use entity::entities::{transaction_link, transaction_link_member};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::kafka::envelope::transaction_uuid;
use crate::kafka::links::TransactionLinkRecord;

/// Upserts (`Some`) or deletes (`None`) the link `link_id` and its members.
///
/// Members are addressed by `(source, external_id)` and mapped to the
/// deterministic transaction id, exactly as the projector does on a replay.
pub async fn project_transaction_link(
    txn: &impl ConnectionTrait,
    link_id: Uuid,
    record: Option<TransactionLinkRecord>,
) -> Result<(), DbErr> {
    let ids = record.as_ref().map(|r| {
        r.members.iter().map(|m| transaction_uuid(&m.source, &m.external_id)).collect::<Vec<_>>()
    });
    project_transaction_link_with_ids(txn, link_id, record, ids.as_deref().unwrap_or(&[])).await
}

/// As [`project_transaction_link`], with the member transaction ids given
/// (parallel to `record.members`). The GraphQL mutations use this with the
/// ids of the rows they just loaded, which equal the derived ones for every
/// imported transaction but need not for a hand-seeded row.
pub async fn project_transaction_link_with_ids(
    txn: &impl ConnectionTrait,
    link_id: Uuid,
    record: Option<TransactionLinkRecord>,
    member_ids: &[Uuid],
) -> Result<(), DbErr> {
    let Some(record) = record else {
        delete_link(txn, link_id).await?;
        return Ok(());
    };

    if let Some(existing) = transaction_link::Entity::find_by_id(link_id).one(txn).await?
        && existing.revision > record.revision
    {
        return Ok(());
    }

    let model = transaction_link::ActiveModel {
        id: Set(link_id),
        kind: Set(record.kind.as_str().to_string()),
        owner_user_id: Set(record.owner_user_id),
        note: Set(record.note.clone()),
        revision: Set(record.revision.into()),
    };
    transaction_link::Entity::insert(model)
        .on_conflict(
            OnConflict::column(transaction_link::Column::Id)
                .update_columns([
                    transaction_link::Column::Kind,
                    transaction_link::Column::OwnerUserId,
                    transaction_link::Column::Note,
                    transaction_link::Column::Revision,
                ])
                .to_owned(),
        )
        .exec(txn)
        .await?;

    transaction_link_member::Entity::delete_many()
        .filter(transaction_link_member::Column::LinkId.eq(link_id))
        .exec(txn)
        .await?;
    let members: Vec<transaction_link_member::ActiveModel> = record
        .members
        .iter()
        .zip(member_ids)
        .map(|(m, id)| transaction_link_member::ActiveModel {
            link_id: Set(link_id),
            transaction_id: Set(*id),
            role: Set(m.role.as_str().to_string()),
        })
        .collect();
    if !members.is_empty() {
        transaction_link_member::Entity::insert_many(members)
            .on_conflict(
                OnConflict::columns([
                    transaction_link_member::Column::LinkId,
                    transaction_link_member::Column::TransactionId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .do_nothing()
            .exec(txn)
            .await?;
    }
    Ok(())
}

async fn delete_link(txn: &impl ConnectionTrait, link_id: Uuid) -> Result<(), DbErr> {
    transaction_link_member::Entity::delete_many()
        .filter(transaction_link_member::Column::LinkId.eq(link_id))
        .exec(txn)
        .await?;
    transaction_link::Entity::delete_by_id(link_id).exec(txn).await?;
    Ok(())
}
