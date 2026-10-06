//! Persisted consumer offsets (§2.3). The projector never joins a Kafka
//! consumer group: offsets live in `projection_offset`, written in the same
//! transaction as the rows they cover, and are what `assign()` starts from.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::entities::projection_offset;
use sea_orm::{
    sea_query::OnConflict, ActiveValue::Set, ConnectionTrait, DbErr, EntityTrait,
};

/// `next_offset` to resume from, per `(topic, partition)`. Absent ⇒
/// `Offset::Beginning` (§2.3) — the caller decides that, this module only
/// reports what is on record.
pub async fn load_offsets(
    db: &impl ConnectionTrait,
) -> Result<HashMap<(String, i32), i64>, DbErr> {
    let rows = projection_offset::Entity::find().all(db).await?;
    Ok(rows
        .into_iter()
        .map(|row| ((row.topic, row.partition), row.next_offset))
        .collect())
}

/// Upserts the resume point for one `(topic, partition)`. Called inside the
/// same transaction as the batch's row writes (§2.3) — committing both
/// together is what makes "offset moved" and "rows written" atomic.
pub async fn commit_offset(
    txn: &impl ConnectionTrait,
    topic: &str,
    partition: i32,
    next_offset: i64,
    updated_at: DateTime<Utc>,
) -> Result<(), DbErr> {
    let model = projection_offset::ActiveModel {
        topic: Set(topic.to_string()),
        partition: Set(partition),
        next_offset: Set(next_offset),
        updated_at: Set(updated_at.into()),
    };

    projection_offset::Entity::insert(model)
        .on_conflict(
            OnConflict::columns([
                projection_offset::Column::Topic,
                projection_offset::Column::Partition,
            ])
            .update_columns([
                projection_offset::Column::NextOffset,
                projection_offset::Column::UpdatedAt,
            ])
            .to_owned(),
        )
        .exec(txn)
        .await?;

    Ok(())
}
