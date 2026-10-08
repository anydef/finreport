//! The `learning_exemption` table, a projection of
//! `finreport.learning-exemption`: merchants (by normalised
//! `counterparty_key`) the user has said no rule may be learned for. No
//! foreign keys (iteration 2 §3); `decided_by` is a plain audit value.
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(LearningExemption::Table)
                    .if_not_exists()
                    .col(text(LearningExemption::CounterpartyKey).primary_key())
                    .col(uuid_null(LearningExemption::DecidedBy))
                    .col(timestamp_with_time_zone(LearningExemption::Revision))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(LearningExemption::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum LearningExemption {
    Table,
    CounterpartyKey,
    DecidedBy,
    Revision,
}
