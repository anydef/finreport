//! Projection of `finreport.display-alias` into the `display_alias` table.
//!
//! Same posture as every other labeling projection: an upsert guarded by the
//! record's `revision` (a stale record is declined, last writer wins), and a
//! tombstone deletes the row so the original name shows again.

use entity::entities::display_alias;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::kafka::labeling::{AliasKind, DisplayAliasRecord};

/// Upserts `record`, or on `None` deletes the alias at `(user_id, kind, key)`.
pub async fn project_display_alias(
    txn: &impl ConnectionTrait,
    user_id: Uuid,
    kind: AliasKind,
    key: &str,
    record: Option<DisplayAliasRecord>,
) -> Result<(), DbErr> {
    let Some(record) = record else {
        display_alias::Entity::delete_many()
            .filter(display_alias::Column::UserId.eq(user_id))
            .filter(display_alias::Column::Kind.eq(kind.as_str()))
            .filter(display_alias::Column::Key.eq(key))
            .exec(txn)
            .await?;
        return Ok(());
    };

    let model = display_alias::ActiveModel {
        user_id: Set(record.user_id),
        kind: Set(record.kind.as_str().to_string()),
        key: Set(record.key),
        alias: Set(record.alias),
        revision: Set(record.revision.into()),
    };
    let mut on_conflict = OnConflict::columns([
        display_alias::Column::UserId,
        display_alias::Column::Kind,
        display_alias::Column::Key,
    ]);
    on_conflict
        .update_columns([display_alias::Column::Alias, display_alias::Column::Revision])
        .action_cond_where(
            Expr::col((display_alias::Entity, display_alias::Column::Revision))
                .lte(Expr::cust("excluded.revision")),
        );
    match display_alias::Entity::insert(model)
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
