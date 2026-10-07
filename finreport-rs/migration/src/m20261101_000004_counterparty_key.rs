//! Iteration 2 §3/§2.5: `transaction.counterparty_key`, a *derived*
//! projector-owned column (iteration 1 §1 explicitly allows these) filled by
//! the normalization step so rules and the learner can index on it directly
//! instead of recomputing normalization at query time.
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Transaction::Table)
                    .add_column(text_null(Transaction::CounterpartyKey))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_transaction_counterparty_key")
                    .table(Transaction::Table)
                    .col(Transaction::CounterpartyKey)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Transaction::Table)
                    .drop_column(Transaction::CounterpartyKey)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Transaction {
    Table,
    CounterpartyKey,
}
