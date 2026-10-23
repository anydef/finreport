//! `purchase_order_split`: the record that a transaction's split was made by
//! applying an Amazon order (iteration 6 slice 3), and what that split was.
//!
//! The split itself lives in `transaction_split` like any other. This table
//! exists so the system can tell **its own split from a user's**: undo only
//! ever removes a split whose current parts still equal `parts` here, and a
//! second apply is recognised as "already done". `parts` also keeps the item
//! title per part, because a split part has no description column.
//!
//! One row per transaction (the primary key), so two concurrent applies cannot
//! both claim it. Deliberately not a projection of a topic and not truncated
//! by `dev-reset`: a row that no longer matches the live split is detected and
//! discarded rather than trusted.
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PurchaseOrderSplit::Table)
                    .if_not_exists()
                    .col(uuid(PurchaseOrderSplit::TransactionId).primary_key())
                    .col(uuid(PurchaseOrderSplit::UserId))
                    .col(string(PurchaseOrderSplit::Source))
                    .col(string(PurchaseOrderSplit::OrderId))
                    .col(json_binary(PurchaseOrderSplit::Parts))
                    .col(timestamp_with_time_zone(PurchaseOrderSplit::AppliedAt))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_purchase_order_split_order")
                    .table(PurchaseOrderSplit::Table)
                    .col(PurchaseOrderSplit::UserId)
                    .col(PurchaseOrderSplit::Source)
                    .col(PurchaseOrderSplit::OrderId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PurchaseOrderSplit::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PurchaseOrderSplit {
    Table,
    TransactionId,
    UserId,
    Source,
    OrderId,
    Parts,
    AppliedAt,
}
