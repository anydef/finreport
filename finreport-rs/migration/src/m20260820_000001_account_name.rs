use sea_orm_migration::{prelude::*, schema::*};

use crate::m20220101_000001_account::Account;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Human-readable label of the Comdirect login an account was imported
        // through, so accounts belonging to different logins can be told apart.
        // Purely descriptive — accounts are referenced by `account_id`, never
        // by this. Nullable because a login need not be labelled at all;
        // existing rows stay NULL until the importer next runs.
        manager
            .alter_table(
                Table::alter()
                    .table(Account::Table)
                    .add_column(string_null(Account::AccountName))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Account::Table)
                    .drop_column(Account::AccountName)
                    .to_owned(),
            )
            .await
    }
}
