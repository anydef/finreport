//! The `purchase_order` / `purchase_order_item` tables, a projection of
//! `finreport.purchase-order`: retailer orders (today only Amazon) kept as
//! evidence about bank transactions, never as transactions themselves.
//!
//! The key is `(user_id, source, order_id)`: an order id is only unique per
//! retailer, and two household members can each upload a file containing the
//! same order. There is deliberately **no column** that could hold a
//! recipient name or address - the export's address columns are dropped at
//! the parser boundary and never reach the topic or these tables.
//! No foreign keys (iteration 2 §3); items are replaced wholesale with their
//! order.
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PurchaseOrder::Table)
                    .if_not_exists()
                    .col(uuid(PurchaseOrder::UserId))
                    .col(string(PurchaseOrder::Source))
                    .col(string(PurchaseOrder::OrderId))
                    .col(date(PurchaseOrder::OrderDate))
                    .col(decimal_len(PurchaseOrder::Total, 20, 4))
                    .col(string(PurchaseOrder::Currency))
                    .col(string(PurchaseOrder::PriceBasis))
                    .col(boolean(PurchaseOrder::DiscountsNetted))
                    .col(decimal_len(PurchaseOrder::Remainder, 20, 4))
                    .col(timestamp_with_time_zone(PurchaseOrder::Revision))
                    .primary_key(
                        Index::create()
                            .col(PurchaseOrder::UserId)
                            .col(PurchaseOrder::Source)
                            .col(PurchaseOrder::OrderId),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(PurchaseOrderItem::Table)
                    .if_not_exists()
                    .col(uuid(PurchaseOrderItem::UserId))
                    .col(string(PurchaseOrderItem::Source))
                    .col(string(PurchaseOrderItem::OrderId))
                    .col(integer(PurchaseOrderItem::Position))
                    .col(string(PurchaseOrderItem::Asin))
                    .col(integer(PurchaseOrderItem::Quantity))
                    .col(decimal_len(PurchaseOrderItem::Price, 20, 4))
                    .col(decimal_len_null(PurchaseOrderItem::Discount, 20, 4))
                    .col(text_null(PurchaseOrderItem::Promotion))
                    .col(decimal_len_null(PurchaseOrderItem::PromotionAmount, 20, 4))
                    .col(text(PurchaseOrderItem::Title))
                    .col(text(PurchaseOrderItem::Status))
                    .col(boolean(PurchaseOrderItem::Returned))
                    .col(decimal_len(PurchaseOrderItem::LineTotal, 20, 4))
                    .primary_key(
                        Index::create()
                            .col(PurchaseOrderItem::UserId)
                            .col(PurchaseOrderItem::Source)
                            .col(PurchaseOrderItem::OrderId)
                            .col(PurchaseOrderItem::Position),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PurchaseOrderItem::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(PurchaseOrder::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PurchaseOrder {
    Table,
    UserId,
    Source,
    OrderId,
    OrderDate,
    Total,
    Currency,
    PriceBasis,
    DiscountsNetted,
    Remainder,
    Revision,
}

#[derive(DeriveIden)]
enum PurchaseOrderItem {
    Table,
    UserId,
    Source,
    OrderId,
    Position,
    Asin,
    Quantity,
    Price,
    Discount,
    Promotion,
    PromotionAmount,
    Title,
    Status,
    Returned,
    LineTotal,
}
