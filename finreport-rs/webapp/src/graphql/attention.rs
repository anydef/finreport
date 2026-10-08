//! `attentionSummary`: what is waiting for the user, in one cheap query.
//!
//! Both buckets are built from the transaction list's own filter machinery
//! (`build_condition_with_category_filters`), so "uncategorised" and "held for
//! review" mean exactly what the `uncategorized` / `needsReview` filters mean
//! and the numbers cannot drift from the list a click lands on. Deliberately
//! **all-time**: the summary takes no date filter, because a period-scoped
//! backlog count would hide the very rows it exists to surface.

use async_graphql::SimpleObject;
use entity::entities::transaction;
use rust_decimal::Decimal as RustDecimal;
use sea_orm::sea_query::Expr;
use sea_orm::{DatabaseConnection, EntityTrait, FromQueryResult, QueryFilter, QuerySelect};
use uuid::Uuid;

use crate::graphql::scalars::Decimal;
use crate::graphql::transactions::build_condition_with_category_filters;
use crate::graphql::types::TransactionFilter;

/// A count of transactions and what they are worth.
#[derive(SimpleObject, Debug)]
pub struct AttentionBucket {
    pub count: i32,
    /// Magnitude of the **signed** net of the transactions: a refund cancels the
    /// charge it reverses (as in the category breakdown). Accounts share a currency.
    pub total_amount: Decimal,
}

#[derive(SimpleObject, Debug)]
pub struct AttentionSummary {
    /// No category assigned (see the `uncategorized` transaction filter).
    pub uncategorized: AttentionBucket,
    /// Labels held for human review (see the `needsReview` filter).
    pub needs_review: AttentionBucket,
}

#[derive(FromQueryResult)]
struct BucketRow {
    count: i64,
    total: RustDecimal,
}

fn zero_bucket() -> AttentionBucket {
    AttentionBucket { count: 0, total_amount: Decimal(RustDecimal::ZERO) }
}

async fn bucket(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
) -> async_graphql::Result<AttentionBucket> {
    let condition = build_condition_with_category_filters(db, scoped_ids, filter).await?;
    let row = transaction::Entity::find()
        .filter(condition)
        .select_only()
        .column_as(Expr::cust("COUNT(*)"), "count")
        .column_as(Expr::cust("ABS(COALESCE(SUM(transaction.amount), 0))"), "total")
        .into_model::<BucketRow>()
        .one(db)
        .await?;
    Ok(match row {
        Some(r) => AttentionBucket { count: r.count as i32, total_amount: Decimal(r.total) },
        None => zero_bucket(),
    })
}

pub async fn fetch_attention_summary(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
) -> async_graphql::Result<AttentionSummary> {
    if scoped_ids.is_empty() {
        return Ok(AttentionSummary { uncategorized: zero_bucket(), needs_review: zero_bucket() });
    }
    let uncategorized = TransactionFilter { uncategorized: Some(true), ..Default::default() };
    let needs_review = TransactionFilter { needs_review: Some(true), ..Default::default() };
    let (uncategorized, needs_review) = tokio::try_join!(
        bucket(db, scoped_ids, &uncategorized),
        bucket(db, scoped_ids, &needs_review),
    )?;
    tracing::debug!(
        uncategorized = uncategorized.count,
        needs_review = needs_review.count,
        "attention summary"
    );
    Ok(AttentionSummary { uncategorized, needs_review })
}
