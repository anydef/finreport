//! Iteration 2 §3: the `category` table, a projection of `finreport.category`
//! (§2.2). `slug` is the identity the event log and the LLM cache key against,
//! so it never changes; renaming a category edits `name` only. No foreign key
//! on `parent_id`: a rebuild can project a child before its parent lands in
//! the same batch (§3 "No foreign keys on these five tables" applies to the
//! self-reference here too, for the same reason).
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Category::Table)
                    .if_not_exists()
                    .col(pk_uuid(Category::Id))
                    .col(text_uniq(Category::Slug))
                    .col(uuid_null(Category::ParentId))
                    .col(text(Category::Name))
                    .col(text(Category::Kind))
                    .col(small_integer(Category::Depth))
                    .col(integer(Category::SortOrder).default(0))
                    .col(boolean(Category::Archived).default(false))
                    .col(text(Category::Origin))
                    .col(uuid_null(Category::OwnerUserId))
                    .col(timestamp_with_time_zone(Category::Revision))
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE category ADD CONSTRAINT chk_category_depth \
                 CHECK (depth BETWEEN 1 AND 3)",
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_category_parent_id")
                    .table(Category::Table)
                    .col(Category::ParentId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Category::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum Category {
    Table,
    Id,
    Slug,
    ParentId,
    Name,
    Kind,
    Depth,
    SortOrder,
    Archived,
    Origin,
    OwnerUserId,
    Revision,
}
