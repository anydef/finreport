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

/// §4 "Totals exclude transfers": household-level form of the rule — every
/// flagged row is dropped, whatever its counterpart. Transfers have no
/// user-override layer (§2.2), so this reads `transaction_insight` directly.
/// `id_expr` is the (possibly aliased) `transaction.id` column reference in
/// the query this is spliced into.
///
/// The cashflow aggregations use the *scope-aware* variant
/// ([`scoped_transfer_exclusion_sql`]) instead; this one stays for callers
/// that genuinely aggregate the whole household and have no selection of
/// their own (goal evaluation, `goals::rows`).
pub(crate) fn transfer_exclusion_sql(id_expr: &str) -> String {
    format!(
        " AND COALESCE((SELECT ti.is_transfer FROM transaction_insight ti WHERE ti.transaction_id = {id_expr}), false) = false"
    )
}

/// The account set a flagged transfer's counterpart is tested against.
///
/// **Two different account sets are in play and confusing them silently
/// reverts this fix:**
///
/// - `scoped_ids` is the *authorisation* scope — every account the caller may
///   see. It is not the selection.
/// - `filter.account_ids` (`TransactionFilter.accountIds`) is the *selected
///   subset*.
///
/// The effective scope is the selection when there is one, else the whole
/// authorisation scope. Writing the transfer rule against `scoped_ids` alone
/// would be a no-op for the very case this fix exists for: when the user
/// drills into one of their two accounts, both accounts are still in
/// `scoped_ids`, so both legs would still look "inside scope" and still be
/// dropped — the phantom deficit. Do not "simplify" this away.
///
/// (`cashflowSummary`/`cashflowGraph` happen to pre-narrow `scoped_ids`
/// through `scoped_account_ids`, so today the two agree there; this helper
/// makes the aggregation correct regardless of what a caller passes, and is
/// the single place the choice is made.)
pub(crate) fn effective_scope_ids(scoped_ids: &[Uuid], filter: &TransactionFilter) -> Vec<Uuid> {
    match filter.account_ids.as_ref() {
        Some(ids) if !ids.is_empty() => ids.iter().map(|id| id.0).collect(),
        _ => scoped_ids.to_vec(),
    }
}

/// Renders an account-id list as a SQL `uuid[]` literal.
///
/// Inlined rather than bound as a parameter because this predicate is spliced
/// into four queries whose positional-parameter indices are tracked
/// independently (and one of which repeats the fragment twice). The values are
/// already-parsed [`Uuid`]s, whose `Display` is hex-and-dashes only, so there
/// is nothing to escape.
fn uuid_array_literal(ids: &[Uuid]) -> String {
    let items: Vec<String> = ids.iter().map(|id| format!("'{id}'::uuid")).collect();
    format!("ARRAY[{}]::uuid[]", items.join(", "))
}

/// §4, scope-aware: drop a flagged transfer leg **unless its counterpart is
/// provably outside `scope_ids`**.
///
/// Excluding transfers is right at household level — the money did not leave —
/// and wrong per account, where an inbound transfer *is* that account's
/// income. `detect::transfer` flags **both** legs, so the unscoped rule
/// dropped the receiving account's income too and the account read as a
/// deficit although the money had arrived.
///
/// A leg is therefore included only when we can *prove* the other side lies
/// outside the scope: `transfer_counterpart_id IS NOT NULL` **and** that
/// transaction's `account_id` is not in `scope_ids`. A NULL
/// `transfer_counterpart_id` proves nothing, so such a row stays excluded —
/// a deliberate conservative choice, not an oversight: unknown data must not
/// flip a figure the user has already looked at.
///
/// With every account selected (the dashboard default) both legs are inside
/// the scope, so both are still dropped and every existing figure is
/// unchanged by construction.
///
/// `transfer_counterpart_id` is a *transaction* id, so the counterpart's
/// account is reached by going back to `transaction`.
///
/// **That lookup is a nested scalar subquery, not a `JOIN transaction ct` in
/// the `EXISTS` arm, and it must stay one.** Call sites splice an
/// `id_expr` that may be unqualified (`"id"`, where the outer `FROM` is
/// `transaction`); a `transaction` in the subquery's own `FROM` shadows it,
/// so `ti.transaction_id = id` would silently become
/// `ti.transaction_id = ct.id` — never true, and the fix becomes a no-op.
/// Only `transaction_insight` appears in the correlated query's `FROM`, and
/// it has no `id` column, so `id_expr` resolves outward as intended.
///
/// The `COALESCE(..., true)` is what keeps a NULL `transfer_counterpart_id`
/// excluded: no counterpart row means "treat it as inside the scope".
pub(crate) fn scoped_transfer_exclusion_sql(id_expr: &str, scope_ids: &[Uuid]) -> String {
    // No scope at all: nothing can be proven outside it, so fall back to the
    // household rule rather than letting `= ANY(ARRAY[]::uuid[])` (always
    // false) include every flagged leg.
    if scope_ids.is_empty() {
        return transfer_exclusion_sql(id_expr);
    }
    let scope = uuid_array_literal(scope_ids);
    format!(
        " AND (COALESCE((SELECT ti.is_transfer FROM transaction_insight ti \
           WHERE ti.transaction_id = {id_expr}), false) = false \
         OR EXISTS (SELECT 1 FROM transaction_insight ti \
           WHERE ti.transaction_id = {id_expr} AND ti.is_transfer = true \
             AND NOT COALESCE((SELECT ct.account_id = ANY({scope}) FROM transaction ct \
               WHERE ct.id = ti.transfer_counterpart_id), true)))"
    )
}

/// Same rule, inverted: only transfers, for `filter.transfer = true`. Not
/// scope-aware — "show me only transfers" means the same thing it always did.
pub(crate) fn transfer_inclusion_sql(id_expr: &str) -> String {
    format!(
        " AND COALESCE((SELECT ti.is_transfer FROM transaction_insight ti WHERE ti.transaction_id = {id_expr}), false) = true"
    )
}

/// Picks [`transfer_inclusion_sql`] or [`scoped_transfer_exclusion_sql`] per
/// §4's rule (default/explicit-false excludes, explicit-true includes only).
pub(crate) fn transfer_filter_sql(
    id_expr: &str,
    filter: &TransactionFilter,
    scoped_ids: &[Uuid],
) -> String {
    if filter.transfer == Some(true) {
        transfer_inclusion_sql(id_expr)
    } else {
        scoped_transfer_exclusion_sql(id_expr, &effective_scope_ids(scoped_ids, filter))
    }
}

pub(crate) fn bounded_range(filter: &TransactionFilter) -> GqlResult<(NaiveDate, NaiveDate)> {
    match (filter.start_date, filter.end_date) {
        (Some(start), Some(end)) if start.0 <= end.0 => Ok((start.0, end.0)),
        (Some(start), Some(end)) => Err(async_graphql::Error::new(format!(
            "startDate {start:?} must not be after endDate {end:?}"
        ))
        .extend_with(|_, e| e.set("code", "VALIDATION"))),
        _ => Err(
            async_graphql::Error::new("a date range needs both startDate and endDate")
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
pub(crate) async fn first_currency(db: &DatabaseConnection, scoped_ids: &[Uuid]) -> GqlResult<String> {
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
            " AND {}",
            crate::graphql::transactions::search_sql(
                "transaction",
                filter,
                &format!("${idx}"),
                &format!("${idx}"),
                &format!("${idx}"),
            )
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
    let (id_amount_sql, id_amount_params) =
        crate::graphql::transactions::id_and_amount_sql("transaction", filter, idx);
    sql.push_str(&id_amount_sql);
    params.extend(id_amount_params);
    match filter.has_counterparty {
        Some(true) => sql.push_str(" AND counterparty_name IS NOT NULL"),
        Some(false) => sql.push_str(" AND counterparty_name IS NULL"),
        None => {}
    }
    sql.push_str(&transfer_filter_sql("id", filter, scoped_ids));

    sql.push_str(" GROUP BY account_id, counterparty_name, is_income");
    (sql, params)
}

/// Income-side SQL for the `CATEGORY` dimension (§5): identical to
/// `build_graph_sql` but restricted to `amount > 0`, since the outcome side
/// is queried separately by category via `build_category_outcome_sql`.
fn build_category_income_sql(
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
    range_start: NaiveDate,
    range_end: NaiveDate,
) -> (String, Vec<sea_orm::Value>) {
    let (mut sql, params) = build_graph_sql(scoped_ids, filter, range_start, range_end);
    sql = sql.replace(
        "WHERE account_id = ANY($1)",
        "WHERE amount > 0 AND account_id = ANY($1)",
    );
    (sql, params)
}

/// Outcome side of the `CATEGORY` dimension (§5): groups `amount < 0` rows
/// by resolved category slug instead of counterparty. Split-aware per
/// `categoryBreakdown`'s own rule (§5/`breakdown.rs`) — a transaction with a
/// valid (non-`invalid`) split contributes its parts, one row per category;
/// otherwise the whole transaction counts once, against its
/// `transaction_label` category (or `NULL`, folded into "Uncategorized" by
/// `graph::fold_side`). Same `search`/`counterpartyNames`/`hasCounterparty`
/// predicates as `build_graph_sql`, applied on the `transaction` alias `t`.
fn build_category_outcome_sql(
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
    range_start: NaiveDate,
    range_end: NaiveDate,
) -> (String, Vec<sea_orm::Value>) {
    let mut extra = String::new();
    let mut params: Vec<sea_orm::Value> = vec![
        scoped_ids.to_vec().into(),
        range_start.into(),
        range_end.into(),
    ];
    let mut idx = 4;
    if let Some(search) = filter.search.as_ref().filter(|s| !s.is_empty()) {
        extra.push_str(&format!(
            " AND {}",
            crate::graphql::transactions::search_sql(
                "t",
                filter,
                &format!("${idx}"),
                &format!("${idx}"),
                &format!("${idx}"),
            )
        ));
        params.push(format!("%{search}%").into());
        idx += 1;
    }
    if let Some(names) = filter.counterparty_names.as_ref().filter(|n| !n.is_empty()) {
        extra.push_str(&format!(" AND t.counterparty_name = ANY(${idx})"));
        params.push(names.clone().into());
        idx += 1;
    }
    let (id_amount_sql, id_amount_params) =
        crate::graphql::transactions::id_and_amount_sql("t", filter, idx);
    extra.push_str(&id_amount_sql);
    params.extend(id_amount_params);
    match filter.has_counterparty {
        Some(true) => extra.push_str(" AND t.counterparty_name IS NOT NULL"),
        Some(false) => extra.push_str(" AND t.counterparty_name IS NULL"),
        None => {}
    }
    // §4: a row whose effective transfer flag is true *or* whose resolved
    // category `kind = TRANSFER` is dropped from the aggregation, unless
    // the caller explicitly asked for transfers (`filter.transfer = true`),
    // in which case only such rows are kept.
    //
    // The `is_transfer` half is now scope-aware (see
    // `scoped_transfer_exclusion_sql`): a flagged leg whose counterpart is
    // provably outside the effective scope is real spending for the selected
    // account. The category-`kind` half is untouched — a user who categorises
    // something as a transfer means it, whatever the selection.
    if filter.transfer == Some(true) {
        extra.push_str(
            " AND (COALESCE((SELECT ti.is_transfer FROM transaction_insight ti WHERE ti.transaction_id = t.id), false) = true OR c.kind = 'transfer')",
        );
    } else {
        extra.push_str(&scoped_transfer_exclusion_sql(
            "t.id",
            &effective_scope_ids(scoped_ids, filter),
        ));
        extra.push_str(" AND COALESCE(c.kind, '') != 'transfer'");
    }

    let sql = format!(
        "SELECT t.account_id AS account_id, c.slug AS counterparty_name, \
           false AS is_income, SUM(ABS(t.amount)) AS total_amount \
         FROM transaction t \
         LEFT JOIN transaction_label tl ON tl.transaction_id = t.id AND tl.status = 'resolved' \
         LEFT JOIN category c ON c.id = tl.category_id \
         WHERE t.amount < 0 AND t.account_id = ANY($1) \
           AND t.booking_date >= $2 AND t.booking_date <= $3 \
           AND NOT EXISTS (SELECT 1 FROM transaction_split ts WHERE ts.transaction_id = t.id AND ts.invalid = false) \
           {extra} \
         GROUP BY t.account_id, c.slug \
         UNION ALL \
         SELECT t.account_id AS account_id, c.slug AS counterparty_name, \
           false AS is_income, SUM(ABS(ts.amount)) AS total_amount \
         FROM transaction_split ts \
         JOIN transaction t ON t.id = ts.transaction_id \
         JOIN category c ON c.id = ts.category_id \
         WHERE ts.invalid = false AND ts.amount < 0 AND t.account_id = ANY($1) \
           AND t.booking_date >= $2 AND t.booking_date <= $3 \
           {extra} \
         GROUP BY t.account_id, c.slug"
    );
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
    let category_mode = graph::is_category_dimension(&grouping.dimensions);

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

    let mut flows: Vec<AggregatedFlow> = Vec::new();
    if category_mode {
        let (income_sql, income_params) =
            build_category_income_sql(scoped_ids, filter, range_start, range_end);
        let (outcome_sql, outcome_params) =
            build_category_outcome_sql(scoped_ids, filter, range_start, range_end);
        for (sql, params) in [(income_sql, income_params), (outcome_sql, outcome_params)] {
            let stmt = Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, params);
            let rows = db.query_all(stmt).await?;
            flows.extend(rows_to_flows(&rows, &labels)?);
        }
    } else {
        let (sql, params) = build_graph_sql(scoped_ids, filter, range_start, range_end);
        let stmt = Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, params);
        let rows = db.query_all(stmt).await?;
        flows = rows_to_flows(&rows, &labels)?;
    }

    let result = graph::build_graph(&flows, grouping.max_nodes_per_dimension, category_mode);

    Ok(CashflowGraph {
        nodes: result.nodes,
        links: result.links,
        currency,
        dimensions: grouping.dimensions,
        truncated: result.truncated,
    })
}

fn rows_to_flows(
    rows: &[sea_orm::QueryResult],
    labels: &HashMap<Uuid, String>,
) -> Result<Vec<AggregatedFlow>, sea_orm::DbErr> {
    rows.iter()
        .map(|row| {
            let account_id: Uuid = row.try_get("", "account_id")?;
            let counterparty_name: Option<String> = row.try_get("", "counterparty_name")?;
            let is_income: bool = row.try_get("", "is_income")?;
            let total_amount: Decimal = row.try_get("", "total_amount")?;
            Ok(AggregatedFlow {
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
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::scalars::Uuid as GqlUuid;

    fn uuid(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn filter_with(account_ids: Option<Vec<Uuid>>) -> TransactionFilter {
        TransactionFilter {
            account_ids: account_ids
                .map(|ids| ids.into_iter().map(GqlUuid).collect()),
            ..Default::default()
        }
    }

    #[test]
    fn no_selection_falls_back_to_the_authorisation_scope() {
        let scoped = vec![uuid(1), uuid(2)];
        assert_eq!(
            effective_scope_ids(&scoped, &filter_with(None)),
            vec![uuid(1), uuid(2)]
        );
    }

    #[test]
    fn an_empty_selection_falls_back_to_the_authorisation_scope() {
        let scoped = vec![uuid(1), uuid(2)];
        assert_eq!(
            effective_scope_ids(&scoped, &filter_with(Some(vec![]))),
            vec![uuid(1), uuid(2)]
        );
    }

    #[test]
    fn a_selection_wins_over_the_authorisation_scope() {
        // The whole point of the helper: drilling into one of two accounts
        // must narrow the transfer rule's scope, not keep the household one.
        let scoped = vec![uuid(1), uuid(2)];
        assert_eq!(
            effective_scope_ids(&scoped, &filter_with(Some(vec![uuid(2)]))),
            vec![uuid(2)]
        );
    }

    #[test]
    fn an_empty_scope_keeps_the_household_exclusion() {
        // `= ANY(ARRAY[]::uuid[])` is always false, so a naive render would
        // make every flagged leg "provably outside scope" and include it.
        let sql = scoped_transfer_exclusion_sql("t.id", &[]);
        assert_eq!(sql, transfer_exclusion_sql("t.id"));
    }

    #[test]
    fn the_scoped_exclusion_requires_a_counterpart_outside_the_scope() {
        let sql = scoped_transfer_exclusion_sql("t.id", &[uuid(7)]);
        // A flagged leg survives only via the EXISTS arm, which looks the
        // counterpart's account up and demands it be outside the scope array.
        assert!(sql.contains("ti.is_transfer = true"));
        assert!(sql.contains("WHERE ct.id = ti.transfer_counterpart_id"));
        assert!(sql.contains(&uuid(7).to_string()));
        // A NULL counterpart yields no row, and COALESCE(..., true) then
        // treats it as inside the scope, so it stays excluded.
        assert!(sql.contains("NOT COALESCE((SELECT ct.account_id = ANY("));
        // The correlated query must not pull `transaction` into its own FROM:
        // that would shadow an unqualified `id_expr` and void the whole fix.
        assert!(!sql.contains("JOIN transaction ct"));
    }

    #[test]
    fn only_transfers_is_not_scope_aware() {
        let filter = TransactionFilter {
            transfer: Some(true),
            ..filter_with(Some(vec![uuid(2)]))
        };
        assert_eq!(
            transfer_filter_sql("id", &filter, &[uuid(1), uuid(2)]),
            transfer_inclusion_sql("id")
        );
    }

    #[test]
    fn the_default_filter_excludes_transfers_over_the_whole_scope() {
        let scoped = vec![uuid(1), uuid(2)];
        let sql = transfer_filter_sql("id", &filter_with(None), &scoped);
        assert_eq!(sql, scoped_transfer_exclusion_sql("id", &scoped));
        assert!(sql.contains(&uuid(1).to_string()) && sql.contains(&uuid(2).to_string()));
    }
}
