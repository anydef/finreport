//! Iteration 2 §3: the `rule` table, a projection of `finreport.rule` (§2.2,
//! §2.7). No foreign key on `category_id` for the same interleaved-batch
//! reason as the labels migration. Learned-rule ids are deterministic
//! (`UUIDv5(FINREPORT_NS, "rule\0"+counterparty_key+"\0"+category_slug)`,
//! §2.8), computed by the labeler/learner, not here.
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Rule::Table)
                    .if_not_exists()
                    .col(pk_uuid(Rule::Id))
                    .col(text(Rule::Name))
                    .col(uuid(Rule::CategoryId))
                    .col(json_binary(Rule::Conditions))
                    .col(integer(Rule::Priority).default(0))
                    .col(text(Rule::State))
                    .col(text(Rule::Origin))
                    .col(boolean(Rule::AutoApproved).default(false))
                    .col(boolean(Rule::UserTouched).default(false))
                    .col(decimal_len_null(Rule::Confidence, 4, 3))
                    .col(json_binary_null(Rule::Evidence))
                    .col(timestamp_with_time_zone(Rule::CreatedAt))
                    .col(timestamp_with_time_zone(Rule::Revision))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_rule_state")
                    .table(Rule::Table)
                    .col(Rule::State)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Rule::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum Rule {
    Table,
    Id,
    Name,
    CategoryId,
    Conditions,
    Priority,
    State,
    Origin,
    AutoApproved,
    UserTouched,
    Confidence,
    Evidence,
    CreatedAt,
    Revision,
}
