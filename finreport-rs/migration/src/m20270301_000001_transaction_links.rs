//! `transaction_link` and `transaction_link_member`, a projection of
//! `finreport.transaction-link`: a user-declared tie between transactions
//! that offset one another (a reimbursement and the expense it repays).
//!
//! The link row carries only the declaration (`kind`, `note`, who decided,
//! `revision`); the members hang off it with a `role`. A link is one expense
//! side and one offsetting side, each holding one *or more* transactions, so
//! "one bill repaid in two instalments" and "one transfer covering three
//! bills" are both a single link. **No amounts are stored**: the offset is
//! derived from the member transactions' own amounts at read time, so a link
//! cannot go stale when a transaction is corrected.
//!
//! No foreign keys (iteration 2 §3): `transaction_id` is a plain value, so a
//! link can project before (or outlive) a transaction it names, and a
//! replay in any order converges. `transaction_id` is indexed because the
//! read path asks "which link is this transaction in?" for a whole page.
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(TransactionLink::Table)
                    .if_not_exists()
                    .col(pk_uuid(TransactionLink::Id))
                    .col(text(TransactionLink::Kind))
                    .col(uuid_null(TransactionLink::OwnerUserId))
                    .col(text_null(TransactionLink::Note))
                    .col(timestamp_with_time_zone(TransactionLink::Revision))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(TransactionLinkMember::Table)
                    .if_not_exists()
                    .col(uuid(TransactionLinkMember::LinkId))
                    .col(uuid(TransactionLinkMember::TransactionId))
                    .col(text(TransactionLinkMember::Role))
                    .primary_key(
                        Index::create()
                            .col(TransactionLinkMember::LinkId)
                            .col(TransactionLinkMember::TransactionId),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_transaction_link_member_transaction")
                    .table(TransactionLinkMember::Table)
                    .col(TransactionLinkMember::TransactionId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(TransactionLinkMember::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(TransactionLink::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum TransactionLink {
    Table,
    Id,
    Kind,
    OwnerUserId,
    Note,
    Revision,
}

#[derive(DeriveIden)]
enum TransactionLinkMember {
    Table,
    LinkId,
    TransactionId,
    Role,
}
