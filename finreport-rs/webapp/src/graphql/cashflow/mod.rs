//! `cashflowSummary` / `cashflowGraph` (§5): DB-touching query functions
//! live here; the aggregation/bucketing/Sankey-building logic they call is
//! pure and lives in `summary.rs` / `graph.rs` so it can be unit-tested
//! without Postgres (§8/§9).

pub mod graph;
pub mod summary;

use async_graphql::{ErrorExtensions, Result as GqlResult};
use chrono::NaiveDate;
use entity::entities::account;
use rust_decimal::Decimal;
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Statement};
use std::collections::HashMap;
use uuid::Uuid;

use crate::graphql::scalars::{Date as GqlDate, Decimal as GqlDecimal};
use crate::graphql::types::{
    CashflowBucket, CashflowGraph, CashflowGraphInput, CashflowSummary, CashflowTotals,
    Direction, Granularity, TransactionFilter,
};
use graph::{AggregatedFlow, FlowDirection};
use summary::SparseBucket;

fn bounded_range(filter: &TransactionFilter) -> GqlResult<(NaiveDate, NaiveDate)> {
    match (filter.start_date, filter.end_date) {
        (Some(start), Some(end)) if start.0 <= end.0 => Ok((start.0, end.0)),
        (Some(start), Some(end)) => Err(async_graphql::Error::new(format!(
            "cashflowSummary: startDate {start:?} must not be after endDate {end:?}"
        ))
        .extend_with(|_, e| e.set("code", "VALIDATION"))),
        _ => Err(
            async_graphql::Error::new("cashflowSummary requires both startDate and endDate")
                .extend_with(|_, e| e.set("code", "VALIDATION")),
        ),
    }
}

/// Runs the SQL aggregation and dense-fills the result (§5).
pub async fn fetch_summary(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
    granularity: Granularity,
) -> GqlResult<CashflowSummary> {
    let (range_start, range_end) = bounded_range(filter)?;

    let currency = first_currency(db, scoped_ids).await?;

    if scoped_ids.is_empty() {
        let dense = summary::fill_dense_buckets(&[], range_start, range_end, granularity);
        return Ok(to_graphql_summary(dense, currency));
    }

    let (sql, params) =
        summary::build_summary_sql(scoped_ids, filter, range_start, range_end, granularity);
    let stmt = Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, params);
    let rows = db.query_all(stmt).await?;

    let sparse: Vec<SparseBucket> = rows
        .iter()
        .map(|row| {
            Ok::<_, sea_orm::DbErr>(SparseBucket {
                bucket_start: row.try_get("", "bucket_start")?,
                income: row.try_get("", "income")?,
                spending: row.try_get("", "spending")?,
                transaction_count: row.try_get("", "tx_count")?,
            })
        })
        .collect::<Result<_, _>>()?;

    let dense = summary::fill_dense_buckets(&sparse, range_start, range_end, granularity);
    Ok(to_graphql_summary(dense, currency))
}

fn to_graphql_summary(dense: Vec<summary::DenseBucket>, currency: String) -> CashflowSummary {
    let mut total_income = Decimal::ZERO;
    let mut total_spending = Decimal::ZERO;
    let mut total_count: i64 = 0;

    let buckets: Vec<CashflowBucket> = dense
        .into_iter()
        .map(|b| {
            total_income += b.income;
            total_spending += b.spending;
            total_count += b.transaction_count;
            CashflowBucket {
                start: GqlDate(b.start),
                end: GqlDate(b.end),
                income: GqlDecimal(b.income),
                spending: GqlDecimal(b.spending),
                net: GqlDecimal(b.net),
                transaction_count: b.transaction_count as i32,
            }
        })
        .collect();

    CashflowSummary {
        buckets,
        total: CashflowTotals {
            income: GqlDecimal(total_income),
            spending: GqlDecimal(total_spending),
            net: GqlDecimal(total_income - total_spending),
            transaction_count: total_count as i32,
        },
        currency,
    }
}

/// Iteration 1 assumes one currency per account set; the first seen (§5).
async fn first_currency(db: &DatabaseConnection, scoped_ids: &[Uuid]) -> GqlResult<String> {
    if scoped_ids.is_empty() {
        return Ok("EUR".to_string());
    }
    let currencies: Vec<String> = account::Entity::find()
        .filter(account::Column::Id.is_in(scoped_ids.to_vec()))
        .all(db)
        .await?
        .into_iter()
        .map(|a| a.currency)
        .collect();
    let first = currencies.first().cloned().unwrap_or_else(|| "EUR".to_string());
    if currencies.iter().any(|c| c != &first) {
        tracing::warn!(?currencies, "mixed-currency account set; no conversion applied (§5)");
    }
    Ok(first)
}

/// Builds the SQL for the grouped `(account_id, counterparty_name, sign)`
/// aggregation `cashflowGraph` needs, reusing `TransactionFilter`'s
/// predicates the same way `build_summary_sql` does.
fn build_graph_sql(
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
    range_start: NaiveDate,
    range_end: NaiveDate,
) -> (String, Vec<sea_orm::Value>) {
    let mut sql = String::from(
        "SELECT account_id, counterparty_name, \
         (amount > 0) AS is_income, \
         SUM(ABS(amount)) AS total_amount \
         FROM transaction \
         WHERE account_id = ANY($1) AND booking_date >= $2 AND booking_date <= $3",
    );
    let mut params: Vec<sea_orm::Value> = vec![
        scoped_ids.to_vec().into(),
        range_start.into(),
        range_end.into(),
    ];
    let mut idx = 4;

    if let Some(search) = filter.search.as_ref().filter(|s| !s.is_empty()) {
        sql.push_str(&format!(
            " AND (counterparty_name ILIKE ${idx} OR description ILIKE ${idx})"
        ));
        params.push(format!("%{search}%").into());
        idx += 1;
    }
    match filter.direction {
        Some(Direction::Income) => sql.push_str(" AND amount > 0"),
        Some(Direction::Spending) => sql.push_str(" AND amount < 0"),
        None => {}
    }
    if let Some(names) = filter.counterparty_names.as_ref().filter(|n| !n.is_empty()) {
        sql.push_str(&format!(" AND counterparty_name = ANY(${idx})"));
        params.push(names.clone().into());
        idx += 1;
    }
    let _ = idx;
    match filter.has_counterparty {
        Some(true) => sql.push_str(" AND counterparty_name IS NOT NULL"),
        Some(false) => sql.push_str(" AND counterparty_name IS NULL"),
        None => {}
    }

    sql.push_str(" GROUP BY account_id, counterparty_name, is_income");
    (sql, params)
}

pub async fn fetch_graph(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
    grouping: Option<CashflowGraphInput>,
) -> GqlResult<CashflowGraph> {
    let grouping = grouping.unwrap_or_default();
    graph::validate_dimensions(&grouping.dimensions)?;

    let (range_start, range_end) = bounded_range(filter)?;
    let currency = first_currency(db, scoped_ids).await?;

    if scoped_ids.is_empty() {
        return Ok(CashflowGraph {
            nodes: vec![],
            links: vec![],
            currency,
            dimensions: grouping.dimensions,
            truncated: false,
        });
    }

    let labels: HashMap<Uuid, String> = account::Entity::find()
        .filter(account::Column::Id.is_in(scoped_ids.to_vec()))
        .all(db)
        .await?
        .into_iter()
        .map(|a| (a.id, a.label.unwrap_or_else(|| a.external_id.clone())))
        .collect();

    let (sql, params) = build_graph_sql(scoped_ids, filter, range_start, range_end);
    let stmt = Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, params);
    let rows = db.query_all(stmt).await?;

    let flows: Vec<AggregatedFlow> = rows
        .iter()
        .map(|row| {
            let account_id: Uuid = row.try_get("", "account_id")?;
            let counterparty_name: Option<String> = row.try_get("", "counterparty_name")?;
            let is_income: bool = row.try_get("", "is_income")?;
            let total_amount: Decimal = row.try_get("", "total_amount")?;
            Ok::<_, sea_orm::DbErr>(AggregatedFlow {
                account_id,
                account_label: labels.get(&account_id).cloned().unwrap_or_default(),
                counterparty_name,
                direction: if is_income {
                    FlowDirection::Income
                } else {
                    FlowDirection::Spending
                },
                amount: total_amount,
            })
        })
        .collect::<Result<_, _>>()?;

    let result = graph::build_graph(&flows, grouping.max_nodes_per_dimension);

    Ok(CashflowGraph {
        nodes: result.nodes,
        links: result.links,
        currency,
        dimensions: grouping.dimensions,
        truncated: result.truncated,
    })
}
