//! Persisted consumer offsets (§2.3). The projector never joins a Kafka
//! consumer group: offsets live in `projection_offset`, written in the same
//! transaction as the rows they cover, and are what `assign()` starts from.
//!
//! Rows are scoped by a `group_id` (`APP_projection_group`), the stand-in for a
//! consumer group id: offsets stored under one group are invisible to another,
//! so switching the id replays every topic from the beginning.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::entities::projection_offset;
use sea_orm::{
    sea_query::OnConflict, ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter,
};

/// `next_offset` to resume from for `group_id`, per `(topic, partition)`.
/// Absent ⇒ `Offset::Beginning` (§2.3) — the caller decides that, this module
/// only reports what is on record under that group.
pub async fn load_offsets(
    db: &impl ConnectionTrait,
    group_id: &str,
) -> Result<HashMap<(String, i32), i64>, DbErr> {
    let rows = projection_offset::Entity::find()
        .filter(projection_offset::Column::GroupId.eq(group_id))
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| ((row.topic, row.partition), row.next_offset))
        .collect())
}

/// Upserts `group_id`'s resume point for one `(topic, partition)`. Called inside the
/// same transaction as the batch's row writes (§2.3) — committing both
/// together is what makes "offset moved" and "rows written" atomic.
pub async fn commit_offset(
    txn: &impl ConnectionTrait,
    group_id: &str,
    topic: &str,
    partition: i32,
    next_offset: i64,
    updated_at: DateTime<Utc>,
) -> Result<(), DbErr> {
    let model = projection_offset::ActiveModel {
        group_id: Set(group_id.to_string()),
        topic: Set(topic.to_string()),
        partition: Set(partition),
        next_offset: Set(next_offset),
        updated_at: Set(updated_at.into()),
    };

    projection_offset::Entity::insert(model)
        .on_conflict(
            OnConflict::columns([
                projection_offset::Column::GroupId,
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

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult, Transaction};

    fn sql_of(log: Vec<Transaction>) -> Vec<String> {
        log.iter().map(|t| format!("{t:?}").replace('\\', "")).collect()
    }

    #[tokio::test]
    async fn load_offsets_filters_by_group() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([Vec::<projection_offset::Model>::new()])
            .into_connection();

        let loaded = load_offsets(&db, "replay-2").await.unwrap();

        assert!(loaded.is_empty());
        let log = sql_of(db.into_transaction_log());
        assert!(log[0].contains(r#""group_id" = $1"#), "{}", log[0]);
        assert!(log[0].contains("replay-2"), "{}", log[0]);
    }

    #[tokio::test]
    async fn commit_offset_writes_and_conflicts_on_group_topic_partition() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_exec_results([MockExecResult { last_insert_id: 0, rows_affected: 1 }])
            .into_connection();

        commit_offset(&db, "replay-2", "finreport.transaction", 0, 5, Utc::now())
            .await
            .unwrap();

        let log = sql_of(db.into_transaction_log());
        assert!(log[0].contains(r#"ON CONFLICT ("group_id", "topic", "partition")"#), "{}", log[0]);
        assert!(log[0].contains("replay-2"), "{}", log[0]);
    }
}
