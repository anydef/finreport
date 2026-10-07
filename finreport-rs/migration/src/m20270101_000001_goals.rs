//! Iteration 4 §2.2: the `goal` table, a projection of `finreport.goal`
//! (§2.1). No foreign keys (iteration 2 §3): `owner_user_id` and the scope's
//! category slugs are plain values, so a goal can project before the user or
//! category it names. The scope and period are flattened into columns, with
//! the scope as arrays — it is read whole every time and never joined
//! against. `scope_category_slugs` holds slugs, not ids, so renaming a
//! category does not invalidate a goal.
//!
//! Progress is deliberately not stored (§2.3): it is derived at read time.
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Goal::Table)
                    .if_not_exists()
                    .col(pk_uuid(Goal::Id))
                    .col(uuid(Goal::OwnerUserId))
                    .col(text(Goal::Name))
                    .col(text(Goal::Kind))
                    .col(decimal_len(Goal::Amount, 20, 4))
                    .col(text(Goal::Currency))
                    .col(
                        ColumnDef::new(Goal::ScopeCategorySlugs)
                            .array(ColumnType::Text)
                            .not_null()
                            .default(Expr::cust("'{}'")),
                    )
                    .col(
                        ColumnDef::new(Goal::ScopeTags)
                            .array(ColumnType::Text)
                            .not_null()
                            .default(Expr::cust("'{}'")),
                    )
                    .col(text(Goal::ScopeCombine))
                    .col(text(Goal::ScopeTagCombine))
                    .col(text(Goal::PeriodKind))
                    .col(text_null(Goal::PeriodCadence))
                    .col(date_null(Goal::PeriodStart))
                    .col(date_null(Goal::PeriodEnd))
                    .col(boolean(Goal::Archived).default(false))
                    .col(timestamp_with_time_zone(Goal::Revision))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_goal_owner_archived")
                    .table(Goal::Table)
                    .col(Goal::OwnerUserId)
                    .col(Goal::Archived)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Dropping the table drops its index with it.
        manager
            .drop_table(Table::drop().table(Goal::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum Goal {
    Table,
    Id,
    OwnerUserId,
    Name,
    /// Column `goal_type` (the bare `type` is a SQL keyword).
    #[sea_orm(iden = "goal_type")]
    Kind,
    Amount,
    Currency,
    ScopeCategorySlugs,
    ScopeTags,
    ScopeCombine,
    ScopeTagCombine,
    PeriodKind,
    PeriodCadence,
    PeriodStart,
    PeriodEnd,
    Archived,
    Revision,
}
