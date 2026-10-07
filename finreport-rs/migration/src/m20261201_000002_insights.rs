//! Iteration 3 §2.3: `transaction_insight` — the detector's auto layer for
//! internal transfers and recurring costs (§2.2, §3). One row per
//! transaction; series attributes are denormalized onto each member instead
//! of a second table (§2.2 "one topic means one projection handler"). No
//! foreign keys, same reasoning as `m20261201_000001_tags.rs`.
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(TransactionInsight::Table)
                    .if_not_exists()
                    .col(pk_uuid(TransactionInsight::TransactionId))
                    .col(
                        boolean(TransactionInsight::IsTransfer)
                            .default(false),
                    )
                    .col(uuid_null(TransactionInsight::TransferCounterpartId))
                    .col(text_null(TransactionInsight::TransferMatch))
                    .col(
                        boolean(TransactionInsight::IsRecurring)
                            .default(false),
                    )
                    .col(uuid_null(TransactionInsight::RecurringSeriesId))
                    .col(text_null(TransactionInsight::RecurringCadence))
                    .col(decimal_len_null(
                        TransactionInsight::RecurringMedianAmount,
                        20,
                        4,
                    ))
                    .col(timestamp_with_time_zone(TransactionInsight::DetectedAt))
                    .col(timestamp_with_time_zone(TransactionInsight::Revision))
                    .to_owned(),
            )
            .await?;
        for (name, col) in [
            (
                "idx_transaction_insight_recurring_series_id",
                TransactionInsight::RecurringSeriesId,
            ),
            (
                "idx_transaction_insight_is_transfer",
                TransactionInsight::IsTransfer,
            ),
        ] {
            manager
                .create_index(
                    Index::create()
                        .if_not_exists()
                        .name(name)
                        .table(TransactionInsight::Table)
                        .col(col)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(TransactionInsight::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum TransactionInsight {
    Table,
    TransactionId,
    IsTransfer,
    TransferCounterpartId,
    TransferMatch,
    IsRecurring,
    RecurringSeriesId,
    RecurringCadence,
    RecurringMedianAmount,
    DetectedAt,
    Revision,
}
