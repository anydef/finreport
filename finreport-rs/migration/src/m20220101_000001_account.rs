use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Account::Table)
                    .if_not_exists()
                    .col(pk_auto(Account::Id))
                    .col(string_uniq(Account::AccountId))
                    .col(string(Account::DisplayId))
                    .col(string(Account::AccountType))
                    .col(string_uniq(Account::IBAN))
                    .col(string(Account::BIC))
                    .col(string(Account::Institute))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Account::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum Account {
    Id,
    Table,
    AccountId,
    DisplayId,
    AccountType,
    IBAN,
    BIC,
    Institute
}
