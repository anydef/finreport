//! Scopes `projection_offset` by a consumer group id (`APP_projection_group`).
//!
//! Offsets stay in Postgres (written in the same transaction as the rows they
//! cover), but are now keyed `(group_id, topic, partition)`: a consumer that
//! starts under a group id with no rows reads from the beginning, which makes
//! replay a configuration change instead of a `DELETE`.
//!
//! Existing rows take the column default `'default'`, the same value
//! `APP_projection_group` defaults to, so an untouched deploy resumes exactly
//! where it left off.
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // `ADD COLUMN ... DEFAULT` backfills existing rows with 'default'.
        // The primary key is swapped in the same (transactional) migration so
        // there is no window where two groups could collide on the old key.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE projection_offset \
                     ADD COLUMN group_id TEXT NOT NULL DEFAULT 'default', \
                     DROP CONSTRAINT projection_offset_pkey, \
                     ADD PRIMARY KEY (group_id, topic, partition)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Only the 'default' group maps back onto the old `(topic, partition)`
        // key; rows of other groups would violate it, so they are dropped.
        manager
            .get_connection()
            .execute_unprepared(
                "DELETE FROM projection_offset WHERE group_id <> 'default'; \
                 ALTER TABLE projection_offset \
                     DROP CONSTRAINT projection_offset_pkey, \
                     DROP COLUMN group_id, \
                     ADD PRIMARY KEY (topic, partition)",
            )
            .await?;
        Ok(())
    }
}
