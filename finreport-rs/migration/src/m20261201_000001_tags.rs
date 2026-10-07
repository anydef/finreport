//! Iteration 3 §2.3: the tags projection, plus the recurring override column
//! on iteration 2's `transaction_user_label` (§2.1/§2.2 — both tags and the
//! recurring override reuse `finreport.user-label`, so the override column
//! lives next to `category_id`/`note`, not in a new table). No foreign keys,
//! same reasoning as the iteration-2 labeling projections (§2.3 there):
//! interleaved batches may project a tag before its transaction.
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(TransactionTag::Table)
                    .if_not_exists()
                    .col(uuid(TransactionTag::TransactionId))
                    .col(text(TransactionTag::Tag))
                    .col(timestamp_with_time_zone(TransactionTag::Revision))
                    .primary_key(
                        Index::create()
                            .col(TransactionTag::TransactionId)
                            .col(TransactionTag::Tag),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_transaction_tag_tag")
                    .table(TransactionTag::Table)
                    .col(TransactionTag::Tag)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(TransactionUserLabel::Table)
                    .add_column(boolean_null(TransactionUserLabel::Recurring))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(TransactionUserLabel::Table)
                    .drop_column(TransactionUserLabel::Recurring)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(TransactionTag::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum TransactionTag {
    Table,
    TransactionId,
    Tag,
    Revision,
}

/// Only the one new column; the rest of `transaction_user_label` is owned by
/// `m20261101_000002_labels.rs`.
#[derive(DeriveIden)]
enum TransactionUserLabel {
    Table,
    Recurring,
}
