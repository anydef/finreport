//! The `account_savings` table, a projection of `finreport.account-savings`:
//! the accounts a user has marked as savings. A row's presence is the flag;
//! a tombstone deletes it. Per user, so the owner is part of the primary key
//! (the same account can be savings to one household member and not another).
//! No foreign keys (iteration 2 §3).
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AccountSavings::Table)
                    .if_not_exists()
                    .col(uuid(AccountSavings::UserId))
                    .col(uuid(AccountSavings::AccountId))
                    .col(timestamp_with_time_zone(AccountSavings::Revision))
                    .primary_key(
                        Index::create()
                            .col(AccountSavings::UserId)
                            .col(AccountSavings::AccountId),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AccountSavings::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum AccountSavings {
    Table,
    UserId,
    AccountId,
    Revision,
}
