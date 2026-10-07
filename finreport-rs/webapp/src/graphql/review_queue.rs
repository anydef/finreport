//! `reviewQueue` (§5): `status = NEEDS_REVIEW` transactions plus
//! `state = IN_REVIEW` rules, scoped to the caller's accounts.

use entity::entities::{transaction, transaction_label};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, Order, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;

use crate::graphql::labels::{prefetch, LabelSplitCache};
use crate::graphql::rules::fetch_rules;
use crate::graphql::scalars::{Date as GqlDate, Decimal as GqlDecimal, Uuid as GqlUuid};
use crate::graphql::transactions::clamp_limit;
use crate::graphql::types::{ReviewQueue, RuleState, Transaction};

fn to_graphql_transaction(row: transaction::Model) -> Transaction {
    Transaction {
        id: GqlUuid(row.id),
        account_id: GqlUuid(row.account_id),
        source: row.source,
        external_id: row.external_id,
        booking_date: GqlDate(row.booking_date),
        valuta_date: row.valuta_date.map(GqlDate),
        booking_status: row.booking_status,
        amount: GqlDecimal(row.amount),
        currency: row.currency,
        counterparty_name: row.counterparty_name,
        counterparty_iban: row.counterparty_iban,
        description: row.description,
        transaction_type: row.transaction_type,
    }
}

pub async fn fetch_review_queue(
    db: &DatabaseConnection,
    cache: &LabelSplitCache,
    scoped_ids: &[Uuid],
    limit: i32,
    offset: i32,
) -> async_graphql::Result<ReviewQueue> {
    let limit = clamp_limit(limit);
    let offset = offset.max(0) as u64;

    if scoped_ids.is_empty() {
        return Ok(ReviewQueue {
            transactions: vec![],
            pending_rules: vec![],
            total_count: 0,
        });
    }

    // `transaction_label.status = 'needs_review'`, scoped via a join on the
    // caller's accounts (§5) — a raw `IN` subquery, since no FK relation
    // exists between `transaction_label` and `transaction` to build a
    // `sea_orm` join against (see `rules.rs`'s `reapply_rule`).
    let needs_review_ids: Vec<Uuid> = transaction_label::Entity::find()
        .filter(transaction_label::Column::Status.eq("needs_review"))
        .all(db)
        .await?
        .into_iter()
        .map(|r| r.transaction_id)
        .collect();

    let total_count = transaction::Entity::find()
        .filter(transaction::Column::Id.is_in(needs_review_ids.clone()))
        .filter(transaction::Column::AccountId.is_in(scoped_ids.to_vec()))
        .count(db)
        .await?;

    let rows = transaction::Entity::find()
        .filter(transaction::Column::Id.is_in(needs_review_ids))
        .filter(transaction::Column::AccountId.is_in(scoped_ids.to_vec()))
        .order_by(transaction::Column::BookingDate, Order::Desc)
        .order_by(transaction::Column::ExternalId, Order::Desc)
        .limit(limit)
        .offset(offset)
        .all(db)
        .await?;

    let transaction_ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    prefetch(db, cache, &transaction_ids).await?;

    let transactions = rows.into_iter().map(to_graphql_transaction).collect();
    let pending_rules = fetch_rules(db, Some(RuleState::InReview)).await?;

    Ok(ReviewQueue {
        transactions,
        pending_rules,
        total_count: total_count as i32,
    })
}
