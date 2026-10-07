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

/// Extends `build_condition` with the iteration-2 `categorySlugs`/
/// `uncategorized`/`needsReview`/`labelSources` filters (§5). Split into a
/// separate, `db`-touching function because resolving `categorySlugs`
/// (which must include descendants) needs a lookup against the `category`
/// table — the base `build_condition` stays synchronous/pure so
/// `cashflow`'s hand-rolled SQL can keep mirroring just its predicates.
pub async fn build_condition_with_category_filters(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
) -> async_graphql::Result<Condition> {
    let mut condition = build_condition(scoped_ids, filter);

    if let Some(slugs) = filter.category_slugs.as_ref().filter(|s| !s.is_empty()) {
        let ids = category_descendant_ids(db, slugs).await?;
        condition = condition.add(category_match_condition(&ids));
    }
    if filter.uncategorized == Some(true) {
        condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
            "NOT EXISTS (SELECT 1 FROM transaction_label tl WHERE tl.transaction_id = transaction.id)",
            Vec::<sea_orm::Value>::new(),
        ));
    }
    if filter.needs_review == Some(true) {
        condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
            "EXISTS (SELECT 1 FROM transaction_label tl WHERE tl.transaction_id = transaction.id AND tl.status = 'needs_review')",
            Vec::<sea_orm::Value>::new(),
        ));
    }
    if let Some(sources) = filter.label_sources.as_ref().filter(|s| !s.is_empty()) {
        let source_strs: Vec<String> = sources.iter().map(|s| gql_label_source_str(*s).to_string()).collect();
        condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
            "EXISTS (SELECT 1 FROM transaction_label tl WHERE tl.transaction_id = transaction.id AND tl.label_source = ANY($1))",
            vec![sea_orm::Value::from(source_strs)],
        ));
    }

    Ok(condition)
}

fn gql_label_source_str(source: crate::graphql::types::LabelSource) -> &'static str {
    match source {
        crate::graphql::types::LabelSource::User => "user",
        crate::graphql::types::LabelSource::Rule => "rule",
        crate::graphql::types::LabelSource::LlmCache => "llm-cache",
        crate::graphql::types::LabelSource::Llm => "llm",
    }
}

/// Every category id matching one of `slugs` or a descendant of one (§5:
/// "OR-ed; includes descendants"). The category table is small (≤ 3 levels)
/// so this loads it once and walks the parent chain in memory rather than
/// building a recursive SQL query.
async fn category_descendant_ids(
    db: &DatabaseConnection,
    slugs: &[String],
) -> async_graphql::Result<Vec<Uuid>> {
    let all = entity::entities::category::Entity::find().all(db).await?;
    let requested_ids: std::collections::HashSet<Uuid> = all
        .iter()
        .filter(|c| slugs.iter().any(|s| s == &c.slug))
        .map(|c| c.id)
        .collect();
    if requested_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(all
        .iter()
        .filter(|c| {
            let mut current = Some(c.id);
            let mut parent_of: std::collections::HashMap<Uuid, Option<Uuid>> = std::collections::HashMap::new();
            for row in &all {
                parent_of.insert(row.id, row.parent_id);
            }
            while let Some(id) = current {
                if requested_ids.contains(&id) {
                    return true;
                }
                current = parent_of.get(&id).copied().flatten();
            }
            false
        })
        .map(|c| c.id)
        .collect())
}

/// A transaction matches a category id set when either its whole-transaction
/// label resolves to one of them, or it has a valid split with a part in
/// one of them (§5's split-aware semantics, mirrored from
/// `breakdown.rs::fetch_breakdown`).
fn category_match_condition(ids: &[Uuid]) -> Condition {
    if ids.is_empty() {
        // No category in the tree matched any requested slug: the filter
        // can never match, same as an empty `IN ()`.
        return Condition::any().add(transaction::Column::Id.eq(Uuid::nil()));
    }
    Condition::any()
        .add(sea_orm::sea_query::Expr::cust_with_values(
            "EXISTS (SELECT 1 FROM transaction_label tl WHERE tl.transaction_id = transaction.id AND tl.category_id = ANY($1))",
            vec![sea_orm::Value::from(ids.to_vec())],
        ))
        .add(sea_orm::sea_query::Expr::cust_with_values(
            "EXISTS (SELECT 1 FROM transaction_split ts WHERE ts.transaction_id = transaction.id AND ts.invalid = false AND ts.category_id = ANY($1))",
            vec![sea_orm::Value::from(ids.to_vec())],
        ))
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

    let condition = build_condition_with_category_filters(db, scoped_ids, filter).await?;
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
