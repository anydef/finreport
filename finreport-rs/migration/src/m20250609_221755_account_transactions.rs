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
                    .table(AccountTransactions::Table)
                    .if_not_exists()
                    .col(pk_auto(AccountTransactions::Id))
                    .col(string_uniq(AccountTransactions::Reference).not_null())
                    .col(string(AccountTransactions::AccountId).not_null())
                    .col(string(AccountTransactions::BookingStatus).not_null())
                    .col(date(AccountTransactions::BookingDate).not_null())
                    .col(double(AccountTransactions::Amount).not_null())
                    .col(string(AccountTransactions::Remitter))
                    .col(string(AccountTransactions::Deptor))
                    .col(string(AccountTransactions::Creditor))
                    .col(string(AccountTransactions::CreditorId))
                    .col(string(AccountTransactions::CreditorMandateId))
                    .col(string(AccountTransactions::RemittanceInfo).not_null())
                    .col(string(AccountTransactions::TransactionType).not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-account-transactions-account-id")
                            .from(AccountTransactions::Table, AccountTransactions::AccountId)
                            .to(Account::Table, Account::AccountId)
                            .on_delete(ForeignKeyAction::Cascade),
                    )

                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AccountTransactions::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum AccountTransactions {
    Table,
    Id,
    Reference,
    AccountId,
    BookingStatus,
    BookingDate,
    Amount,
    Remitter,
    Deptor,
    Creditor,
    CreditorId,
    CreditorMandateId,
    RemittanceInfo,
    TransactionType,
}
