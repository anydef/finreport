use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("IDX_AccountTransactions_AccountId_BookingDate")
                    .table(AccountTransactions::Table)
                    .col(AccountTransactions::AccountId)
                    .col(AccountTransactions::BookingDate)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("IDX_AccountTransactions_AccountId_BookingDate")
                    .table(AccountTransactions::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum AccountTransactions {
    Table,
    AccountId,
    BookingDate,
}
