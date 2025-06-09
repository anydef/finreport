use sea_orm_migration::{prelude::*, schema::*};
use crate::m20220101_000001_account::Account;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AccountBalance::Table)
                    .if_not_exists()
                    .col(pk_auto(AccountBalance::Id))
                    .col(double(AccountBalance::Amount))
                    .col(date(AccountBalance::Date))
                    .col(string(AccountBalance::AccountId).not_null())
                    .index(
                        Index::create()
                            .name("UQ_AccountBalance_AccountId_Date")
                            .col(AccountBalance::AccountId)
                            .col(AccountBalance::Date)
                            .unique(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-account-balance-account-id")
                            .from(AccountBalance::Table, AccountBalance::AccountId)
                            .to(Account::Table, Account::AccountId)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AccountBalance::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum AccountBalance {
    Table,
    Id,
    Amount,
    Date,
    AccountId
}
