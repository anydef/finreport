//! Iteration 2 §3: the four labeling projections. Each is a projection of its
//! own topic (`finreport.transaction-label`, `.user-label`, `.llm-cache` — see
//! §2.2/§2.4/§2.6) and deliberately carries **no foreign keys**: a projector
//! batch interleaves records from different topics, so on a rebuild a label
//! can legitimately arrive before its transaction. An FK would abort that
//! batch, and enough aborted batches exit the process (iteration 1 §2.3).
//! Ids are deterministic (UUIDv5), so the reference resolves the moment the
//! other record lands; resolvers `LEFT JOIN` and treat a missing row as
//! "not projected yet".
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(TransactionLabel::Table)
                    .if_not_exists()
                    .col(pk_uuid(TransactionLabel::TransactionId))
                    .col(uuid_null(TransactionLabel::CategoryId))
                    .col(text(TransactionLabel::LabelSource))
                    .col(uuid_null(TransactionLabel::RuleId))
                    .col(decimal_len_null(TransactionLabel::Confidence, 4, 3))
                    .col(text(TransactionLabel::Status))
                    .col(text_null(TransactionLabel::ReviewReason))
                    .col(text_null(TransactionLabel::ProposedCategoryPath))
                    .col(text_null(TransactionLabel::Provider))
                    .col(text_null(TransactionLabel::Model))
                    .col(text_null(TransactionLabel::PromptVersion))
                    .col(text_null(TransactionLabel::Fingerprint))
                    .col(text_null(TransactionLabel::Reasoning))
                    .col(timestamp_with_time_zone(TransactionLabel::LabeledAt))
                    .to_owned(),
            )
            .await?;
        for (name, col) in [
            ("idx_transaction_label_status", TransactionLabel::Status),
            (
                "idx_transaction_label_category_id",
                TransactionLabel::CategoryId,
            ),
            ("idx_transaction_label_rule_id", TransactionLabel::RuleId),
        ] {
            manager
                .create_index(
                    Index::create()
                        .if_not_exists()
                        .name(name)
                        .table(TransactionLabel::Table)
                        .col(col)
                        .to_owned(),
                )
                .await?;
        }

        manager
            .create_table(
                Table::create()
                    .table(TransactionUserLabel::Table)
                    .if_not_exists()
                    .col(pk_uuid(TransactionUserLabel::TransactionId))
                    .col(uuid_null(TransactionUserLabel::CategoryId))
                    .col(text_null(TransactionUserLabel::Note))
                    .col(timestamp_with_time_zone(TransactionUserLabel::Revision))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(TransactionSplit::Table)
                    .if_not_exists()
                    .col(pk_uuid(TransactionSplit::Id))
                    .col(uuid(TransactionSplit::TransactionId))
                    .col(integer(TransactionSplit::PartIndex))
                    .col(decimal_len(TransactionSplit::Amount, 20, 4))
                    .col(uuid(TransactionSplit::CategoryId))
                    .col(boolean(TransactionSplit::Invalid).default(false))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("uq_transaction_split_transaction_id_part_index")
                    .table(TransactionSplit::Table)
                    .col(TransactionSplit::TransactionId)
                    .col(TransactionSplit::PartIndex)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(LlmLabelCache::Table)
                    .if_not_exists()
                    .col(text(LlmLabelCache::Fingerprint).primary_key())
                    .col(uuid_null(LlmLabelCache::CategoryId))
                    .col(text_null(LlmLabelCache::ProposedPath))
                    .col(decimal_len(LlmLabelCache::Confidence, 4, 3))
                    .col(text(LlmLabelCache::Provider))
                    .col(text(LlmLabelCache::Model))
                    .col(text(LlmLabelCache::PromptVersion))
                    .col(text_null(LlmLabelCache::Reasoning))
                    .col(timestamp_with_time_zone(LlmLabelCache::CreatedAt))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            LlmLabelCache::Table.into_iden(),
            TransactionSplit::Table.into_iden(),
            TransactionUserLabel::Table.into_iden(),
            TransactionLabel::Table.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(table).to_owned())
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum TransactionLabel {
    Table,
    TransactionId,
    CategoryId,
    LabelSource,
    RuleId,
    Confidence,
    Status,
    ReviewReason,
    ProposedCategoryPath,
    Provider,
    Model,
    PromptVersion,
    Fingerprint,
    Reasoning,
    LabeledAt,
}

#[derive(DeriveIden)]
pub enum TransactionUserLabel {
    Table,
    TransactionId,
    CategoryId,
    Note,
    Revision,
}

#[derive(DeriveIden)]
pub enum TransactionSplit {
    Table,
    Id,
    TransactionId,
    PartIndex,
    Amount,
    CategoryId,
    Invalid,
}

#[derive(DeriveIden)]
pub enum LlmLabelCache {
    Table,
    Fingerprint,
    CategoryId,
    ProposedPath,
    Confidence,
    Provider,
    Model,
    PromptVersion,
    Reasoning,
    CreatedAt,
}
