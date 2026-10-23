//! Projection of `finreport.account-savings` into the `account_savings`
//! table. A row's presence is the flag: an upsert guarded by the record's
//! `revision` (a stale record is declined), and a tombstone deletes the row.

use entity::entities::account_savings;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::kafka::savings::AccountSavingsRecord;

/// Upserts `record`, or on `None` clears the flag at `(user_id, account_id)`.
pub async fn project_account_savings(
    txn: &impl ConnectionTrait,
    user_id: Uuid,
    account_id: Uuid,
    record: Option<AccountSavingsRecord>,
) -> Result<(), DbErr> {
    let Some(record) = record else {
        account_savings::Entity::delete_many()
            .filter(account_savings::Column::UserId.eq(user_id))
            .filter(account_savings::Column::AccountId.eq(account_id))
            .exec(txn)
            .await?;
        return Ok(());
    };

    let model = account_savings::ActiveModel {
        user_id: Set(record.user_id),
        account_id: Set(record.account_id),
        revision: Set(record.revision.into()),
    };
    let mut on_conflict = OnConflict::columns([
        account_savings::Column::UserId,
        account_savings::Column::AccountId,
    ]);
    on_conflict
        .update_column(account_savings::Column::Revision)
        .action_cond_where(
            Expr::col((account_savings::Entity, account_savings::Column::Revision))
                .lte(Expr::cust("excluded.revision")),
        );
    match account_savings::Entity::insert(model)
        .on_conflict(on_conflict.to_owned())
        .exec(txn)
        .await
    {
        Ok(_) => Ok(()),
        // The conditional WHERE declined a stale record.
        Err(DbErr::RecordNotInserted) => Ok(()),
        Err(e) => Err(e),
    }
}
