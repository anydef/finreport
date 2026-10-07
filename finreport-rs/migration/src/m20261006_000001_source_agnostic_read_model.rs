//! Reshapes the Comdirect-specific read model into a source-agnostic one
//! (iteration 1, §3). The three existing tables are **renamed**, not dropped:
//! `account` / `account_balance` / `account_transactions` become
//! `legacy_account` / `legacy_account_balance` / `legacy_account_transactions`.
//! They stay untouched as the `legacy-backfill` source and the verification
//! baseline, and are dropped only in a later iteration (§2.8).
//!
//! The new tables this migration creates are owned by the projector: every
//! row carries `origin` ('source' | 'legacy' | 'stub') so a raw record always
//! wins over a reconstructed one, and ids are deterministic (UUIDv5) so a
//! full rebuild never changes a row's identity (§2.3).
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (from, to) in [
            ("account", "legacy_account"),
            ("account_balance", "legacy_account_balance"),
            ("account_transactions", "legacy_account_transactions"),
        ] {
            manager
                .rename_table(
                    Table::rename()
                        .table(Alias::new(from), Alias::new(to))
                        .to_owned(),
                )
                .await?;
        }

        manager
            .create_table(
                Table::create()
                    .table(Account::Table)
                    .if_not_exists()
                    .col(pk_uuid(Account::Id))
                    .col(text(Account::Source))
                    .col(text(Account::ExternalId))
                    .col(text_null(Account::DisplayId))
                    .col(text_null(Account::AccountType))
                    .col(text_null(Account::Iban))
                    .col(text_null(Account::Bic))
                    .col(text_null(Account::Institute))
                    .col(text_null(Account::Label))
                    .col(text(Account::Currency).default("EUR"))
                    .col(json_binary_null(Account::RawPayload))
                    .col(text(Account::Origin))
                    .col(timestamp_with_time_zone(Account::FirstSeenAt))
                    .col(timestamp_with_time_zone(Account::UpdatedAt))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("uq_account_source_external_id")
                    .table(Account::Table)
                    .col(Account::Source)
                    .col(Account::ExternalId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_account_iban")
                    .table(Account::Table)
                    .col(Account::Iban)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(AccountBalance::Table)
                    .if_not_exists()
                    .col(pk_uuid(AccountBalance::Id))
                    .col(uuid(AccountBalance::AccountId))
                    .col(date(AccountBalance::BalanceDate))
                    .col(decimal_len(AccountBalance::Amount, 20, 4))
                    .col(text(AccountBalance::Currency))
                    .col(json_binary_null(AccountBalance::RawPayload))
                    .col(text(AccountBalance::Origin))
                    .col(timestamp_with_time_zone(AccountBalance::ObservedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_account_balance_account_id")
                            .from(AccountBalance::Table, AccountBalance::AccountId)
                            .to(Account::Table, Account::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("uq_account_balance_account_date")
                    .table(AccountBalance::Table)
                    .col(AccountBalance::AccountId)
                    .col(AccountBalance::BalanceDate)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Transaction::Table)
                    .if_not_exists()
                    .col(pk_uuid(Transaction::Id))
                    .col(uuid(Transaction::AccountId))
                    .col(text(Transaction::Source))
                    .col(text(Transaction::ExternalId))
                    .col(date(Transaction::BookingDate))
                    .col(date_null(Transaction::ValutaDate))
                    .col(text(Transaction::BookingStatus))
                    .col(decimal_len(Transaction::Amount, 20, 4))
                    .col(text(Transaction::Currency))
                    .col(text_null(Transaction::CounterpartyName))
                    .col(text_null(Transaction::CounterpartyIban))
                    .col(text_null(Transaction::Description))
                    .col(text_null(Transaction::TransactionType))
                    .col(json_binary(Transaction::RawPayload))
                    .col(text(Transaction::Origin))
                    .col(timestamp_with_time_zone(Transaction::ImportedAt))
                    .col(timestamp_with_time_zone(Transaction::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_transaction_account_id")
                            .from(Transaction::Table, Transaction::AccountId)
                            .to(Account::Table, Account::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("uq_transaction_source_external_id")
                    .table(Transaction::Table)
                    .col(Transaction::Source)
                    .col(Transaction::ExternalId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_transaction_account_booking_date")
                    .table(Transaction::Table)
                    .col(Transaction::AccountId)
                    .col((Transaction::BookingDate, IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_transaction_booking_date")
                    .table(Transaction::Table)
                    .col(Transaction::BookingDate)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ProjectionOffset::Table)
                    .if_not_exists()
                    .col(text(ProjectionOffset::Topic))
                    .col(integer(ProjectionOffset::Partition))
                    .col(big_integer(ProjectionOffset::NextOffset))
                    .col(timestamp_with_time_zone(ProjectionOffset::UpdatedAt))
                    .primary_key(
                        Index::create()
                            .col(ProjectionOffset::Topic)
                            .col(ProjectionOffset::Partition),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ProjectionOffset::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Transaction::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(AccountBalance::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Account::Table).to_owned())
            .await?;

        for (from, to) in [
            ("legacy_account", "account"),
            ("legacy_account_balance", "account_balance"),
            ("legacy_account_transactions", "account_transactions"),
        ] {
            manager
                .rename_table(
                    Table::rename()
                        .table(Alias::new(from), Alias::new(to))
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum Account {
    Table,
    Id,
    Source,
    ExternalId,
    DisplayId,
    AccountType,
    Iban,
    Bic,
    Institute,
    Label,
    Currency,
    RawPayload,
    Origin,
    FirstSeenAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum AccountBalance {
    Table,
    Id,
    AccountId,
    BalanceDate,
    Amount,
    Currency,
    RawPayload,
    Origin,
    ObservedAt,
}

#[derive(DeriveIden)]
enum Transaction {
    Table,
    Id,
    AccountId,
    Source,
    ExternalId,
    BookingDate,
    ValutaDate,
    BookingStatus,
    Amount,
    Currency,
    CounterpartyName,
    CounterpartyIban,
    Description,
    TransactionType,
    RawPayload,
    Origin,
    ImportedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ProjectionOffset {
    Table,
    Topic,
    Partition,
    NextOffset,
    UpdatedAt,
}
