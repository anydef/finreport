use sea_orm_migration::{prelude::*, schema::*};

use crate::m20261006_000002_users::AppUser;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Marks the single bootstrap-managed admin user (webapp's startup
        // bootstrap, see `webapp::auth::bootstrap`), distinct from ordinary
        // `user-admin create-user` accounts. Defaults `false` so every
        // existing row stays non-admin until the bootstrap promotes the one
        // matching `APP_admin_username` (default `admin`).
        manager
            .alter_table(
                Table::alter()
                    .table(AppUser::Table)
                    .add_column(boolean(AppUser::IsAdmin).default(false))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AppUser::Table)
                    .drop_column(AppUser::IsAdmin)
                    .to_owned(),
            )
            .await
    }
}
