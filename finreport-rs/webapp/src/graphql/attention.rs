//! `attentionSummary`: what is waiting for the user, in one cheap query.
//!
//! Both buckets are built from the transaction list's own filter machinery
//! (`build_condition_with_category_filters`), so "uncategorised" and "held for
//! review" mean exactly what the `uncategorized` / `needsReview` filters mean
//! and the numbers cannot drift from the list a click lands on. Deliberately
//! **all-time**: the summary takes no date filter, because a period-scoped
//! backlog count would hide the very rows it exists to surface.

use async_graphql::SimpleObject;
use entity::entities::{rule, transaction};
use rust_decimal::Decimal as RustDecimal;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult, PaginatorTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use crate::graphql::scalars::Decimal;
use crate::graphql::transactions::build_condition_with_category_filters;
use crate::graphql::types::TransactionFilter;
use crate::kafka::labeling::{RuleOrigin, RuleState};
use crate::projection::labeling::{rule_origin_str, rule_state_str};

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
    /// Rules the system learned from the user's own decisions that are live
    /// now: `origin = learned` **and** `state = active`, all-time like the
    /// buckets. Deliberately narrow, so do not widen it: `user`-origin rules
    /// are hand-authored, not learned from decisions; `in_review`, `revoked`
    /// and `rejected` rules label nothing, so counting them would overstate
    /// what the user's decisions achieved. Rules are shared, not per-user, so
    /// this is not scoped to the caller's accounts.
    pub rules_learned: i32,
}

/// One `COUNT(*)` over `rule`; never loads rows.
async fn count_rules_learned(db: &DatabaseConnection) -> async_graphql::Result<i32> {
    let n = rule::Entity::find()
        .filter(rule::Column::Origin.eq(rule_origin_str(RuleOrigin::Learned)))
        .filter(rule::Column::State.eq(rule_state_str(RuleState::Active)))
        .count(db)
        .await?;
    Ok(i32::try_from(n).unwrap_or(i32::MAX))
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
        return Ok(AttentionSummary {
            uncategorized: zero_bucket(),
            needs_review: zero_bucket(),
            rules_learned: count_rules_learned(db).await?,
        });
    }
    let uncategorized = TransactionFilter { uncategorized: Some(true), ..Default::default() };
    let needs_review = TransactionFilter { needs_review: Some(true), ..Default::default() };
    let (uncategorized, needs_review, rules_learned) = tokio::try_join!(
        bucket(db, scoped_ids, &uncategorized),
        bucket(db, scoped_ids, &needs_review),
        count_rules_learned(db),
    )?;
    tracing::debug!(
        uncategorized = uncategorized.count,
        needs_review = needs_review.count,
        rules_learned,
        "attention summary"
    );
    Ok(AttentionSummary { uncategorized, needs_review, rules_learned })
}
