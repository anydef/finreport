//! The `display_alias` table, a projection of `finreport.display-alias`: a
//! user's nickname for a merchant (`kind = 'counterparty'`, key = normalised
//! `counterparty_key`) or for one of their connected accounts (`kind =
//! 'account'`, key = the account id). Per user, so the owner is part of the
//! primary key. No foreign keys (iteration 2 §3).
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DisplayAlias::Table)
                    .if_not_exists()
                    .col(uuid(DisplayAlias::UserId))
                    .col(text(DisplayAlias::Kind))
                    .col(text(DisplayAlias::Key))
                    .col(text(DisplayAlias::Alias))
                    .col(timestamp_with_time_zone(DisplayAlias::Revision))
                    .primary_key(
                        Index::create()
                            .col(DisplayAlias::UserId)
                            .col(DisplayAlias::Kind)
                            .col(DisplayAlias::Key),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DisplayAlias::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum DisplayAlias {
    Table,
    UserId,
    Kind,
    Key,
    Alias,
    Revision,
}
