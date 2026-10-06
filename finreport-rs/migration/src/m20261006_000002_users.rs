//! `app_user`, `user_account` and `user_session` — multi-tenancy and
//! username/password auth (iteration 1, §3/§4). Depends on the new `account`
//! table from `m20261006_000001_source_agnostic_read_model`, so it must run
//! after it.
use sea_orm_migration::{prelude::*, schema::*};

use crate::m20261006_000001_source_agnostic_read_model::Account;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AppUser::Table)
                    .if_not_exists()
                    .col(pk_uuid(AppUser::Id))
                    .col(text_uniq(AppUser::Username))
                    .col(text(AppUser::PasswordHash))
                    .col(text_null(AppUser::DisplayName))
                    .col(boolean(AppUser::Disabled).default(false))
                    .col(timestamp_with_time_zone(AppUser::CreatedAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(UserAccount::Table)
                    .if_not_exists()
                    .col(uuid(UserAccount::UserId))
                    .col(uuid(UserAccount::AccountId))
                    .col(timestamp_with_time_zone(UserAccount::CreatedAt))
                    .primary_key(
                        Index::create()
                            .col(UserAccount::UserId)
                            .col(UserAccount::AccountId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_account_user_id")
                            .from(UserAccount::Table, UserAccount::UserId)
                            .to(AppUser::Table, AppUser::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_account_account_id")
                            .from(UserAccount::Table, UserAccount::AccountId)
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
                    .name("idx_user_account_account_id")
                    .table(UserAccount::Table)
                    .col(UserAccount::AccountId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(UserSession::Table)
                    .if_not_exists()
                    .col(pk_uuid(UserSession::Id))
                    .col(uuid(UserSession::UserId))
                    .col(binary_uniq(UserSession::TokenHash))
                    .col(timestamp_with_time_zone(UserSession::CreatedAt))
                    .col(timestamp_with_time_zone(UserSession::ExpiresAt))
                    .col(timestamp_with_time_zone(UserSession::LastSeenAt))
                    .col(text_null(UserSession::UserAgent))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_session_user_id")
                            .from(UserSession::Table, UserSession::UserId)
                            .to(AppUser::Table, AppUser::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_user_session_expires_at")
                    .table(UserSession::Table)
                    .col(UserSession::ExpiresAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserSession::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(UserAccount::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(AppUser::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum AppUser {
    Table,
    Id,
    Username,
    PasswordHash,
    DisplayName,
    Disabled,
    CreatedAt,
}

#[derive(DeriveIden)]
enum UserAccount {
    Table,
    UserId,
    AccountId,
    CreatedAt,
}

#[derive(DeriveIden)]
enum UserSession {
    Table,
    Id,
    UserId,
    TokenHash,
    CreatedAt,
    ExpiresAt,
    LastSeenAt,
    UserAgent,
}
