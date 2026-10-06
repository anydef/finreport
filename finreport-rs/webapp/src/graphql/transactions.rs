//! `transactions` (§5): filter application, scoping, stable-order
//! pagination.

use entity::entities::transaction;
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, Order, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::graphql::scalars::{Date as GqlDate, Decimal as GqlDecimal, Uuid as GqlUuid};
use crate::graphql::types::{Direction, Transaction, TransactionFilter, TransactionPage};

/// `PageInput.limit`, clamped to `1..=200` (§5). Pure and unit-tested.
pub fn clamp_limit(limit: i32) -> u64 {
    limit.clamp(1, 200) as u64
}

/// Builds the shared `WHERE` condition for both the plain transaction list
/// and (via `cashflow::build_summary_sql`'s hand-rolled SQL mirroring these
/// same predicates) the aggregation queries.
pub fn build_condition(scoped_ids: &[Uuid], filter: &TransactionFilter) -> Condition {
    let mut condition = Condition::all().add(transaction::Column::AccountId.is_in(scoped_ids.to_vec()));

    if let Some(GqlDate(start)) = filter.start_date {
        condition = condition.add(transaction::Column::BookingDate.gte(start));
    }
    if let Some(GqlDate(end)) = filter.end_date {
        condition = condition.add(transaction::Column::BookingDate.lte(end));
    }
    if let Some(search) = filter.search.as_ref().filter(|s| !s.is_empty()) {
        let pattern = format!("%{search}%");
        condition = condition.add(
            Condition::any()
                .add(transaction::Column::CounterpartyName.like(&pattern))
                .add(transaction::Column::Description.like(&pattern)),
        );
    }
    match filter.direction {
        Some(Direction::Income) => {
            condition = condition.add(transaction::Column::Amount.gt(rust_decimal::Decimal::ZERO));
        }
        Some(Direction::Spending) => {
            condition = condition.add(transaction::Column::Amount.lt(rust_decimal::Decimal::ZERO));
        }
        None => {}
    }
    if let Some(names) = filter.counterparty_names.as_ref().filter(|n| !n.is_empty()) {
        condition = condition.add(transaction::Column::CounterpartyName.is_in(names.clone()));
    }
    match filter.has_counterparty {
        Some(true) => condition = condition.add(transaction::Column::CounterpartyName.is_not_null()),
        Some(false) => condition = condition.add(transaction::Column::CounterpartyName.is_null()),
        None => {}
    }

    condition
}

pub async fn fetch_transactions(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
    limit: i32,
    offset: i32,
) -> async_graphql::Result<TransactionPage> {
    let limit = clamp_limit(limit);
    let offset = offset.max(0) as u64;

    if scoped_ids.is_empty() {
        return Ok(TransactionPage {
            items: vec![],
            total_count: 0,
            limit: limit as i32,
            offset: offset as i32,
        });
    }

    let condition = build_condition(scoped_ids, filter);
    let total_count = transaction::Entity::find()
        .filter(condition.clone())
        .count(db)
        .await?;

    let rows = transaction::Entity::find()
        .filter(condition)
        // Stable tie-break (§5): otherwise paging through a day with many
        // transactions can repeat or skip rows.
        .order_by(transaction::Column::BookingDate, Order::Desc)
        .order_by(transaction::Column::ExternalId, Order::Desc)
        .limit(limit)
        .offset(offset)
        .all(db)
        .await?;

    let items = rows
        .into_iter()
        .map(|row| Transaction {
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
        })
        .collect();

    Ok(TransactionPage {
        items,
        total_count: total_count as i32,
        limit: limit as i32,
        offset: offset as i32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_is_clamped_to_the_documented_range() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(-5), 1);
        assert_eq!(clamp_limit(50), 50);
        assert_eq!(clamp_limit(200), 200);
        assert_eq!(clamp_limit(201), 200);
        assert_eq!(clamp_limit(10_000), 200);
    }
}
