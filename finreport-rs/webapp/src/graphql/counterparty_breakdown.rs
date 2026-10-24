//! `counterpartyBreakdown`: what the filtered transactions add up to, per
//! counterparty (merchant / payer), largest magnitude first.
//!
//! # Why a separate query, not a dimension on `categoryBreakdown`
//!
//! `CategoryBreakdownRow.category` is a non-null `Category!`; a counterparty
//! has no category, slug, hierarchy or `kind`. Generalising that row over both
//! dimensions would mean nullable fields every caller has to know when to
//! trust - the drift `graphql/attention.rs` exists to avoid. So this query has
//! its own row type. `CashflowDimension` belongs to the Sankey and has no
//! counterparty member; it is neither extended nor reused here.
//!
//! # Semantics a reader would otherwise guess wrong
//!
//! * **No `kind` argument.** `kind` belongs to a category; filtering by it
//!   would silently drop transactions with no category - exactly the ones a
//!   user hunting for the unlabelled is after. The breakdown covers whatever
//!   `filter` selects, nothing more.
//! * **Transfers are NOT dropped.** `categoryBreakdown` drops `kind: TRANSFER`
//!   by default; this query cannot, and does not special-case transfers.
//!   Pass `transfer: false` in the filter to leave internal transfers out.
//! * **`amount` is the signed net, and there is no `share`.** One counterparty
//!   set mixes signs (employer = income, shop = spending, a refund flips a
//!   shop), so "share of total" is meaningless across it and would invite the
//!   percentages-summing-to-200% bug. A refund nets against its charge because
//!   both rows fall in the same counterparty's `SUM`.
//! * **Bounded output, whole total.** Counterparties are effectively
//!   unlimited, so only the top `limit` by `|amount|` are returned; everything
//!   beyond is one `other` row, so `rows + other` always equals the filtered
//!   set. `other` is `null` when nothing lies beyond `limit`.
//! * **No-key bucket.** Rows whose `counterparty_key` is NULL or empty form one
//!   row with a `null` key, named like `heldMerchantGroups`' bucket.
//!
//! # One filter path
//!
//! The WHERE clause is `build_condition_with_category_filters`, the same helper
//! the transaction list and `attentionSummary` use, so the figures cannot
//! drift from the list a click on a row lands on. Aggregation, ordering and
//! the limit all happen in SQL; no transactions are loaded.

use async_graphql::SimpleObject;
use entity::entities::transaction;
use rust_decimal::Decimal as RustDecimal;
use sea_orm::sea_query::{Expr, Order};
use sea_orm::{DatabaseConnection, EntityTrait, FromQueryResult, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;

use crate::graphql::display_aliases::AliasBook;
use crate::graphql::held_groups::UNGROUPED_NAME;
use crate::graphql::scalars::Decimal;
use crate::graphql::transactions::{build_condition_with_category_filters, clamp_limit};
use crate::graphql::types::TransactionFilter;

#[derive(SimpleObject, Debug, Clone, PartialEq)]
pub struct CounterpartyBreakdownRow {
    /// `null` = the bucket of transactions with no counterparty key.
    pub counterparty_key: Option<String>,
    /// The user's alias for the counterparty if any, else the bank's most
    /// common name for it. The no-key bucket is "No merchant key".
    pub display_name: String,
    /// Signed net of the counterparty's transactions: refunds cancel charges.
    pub amount: Decimal,
    pub transaction_count: i32,
}

#[derive(SimpleObject, Debug)]
pub struct CounterpartyBreakdown {
    /// The top counterparties by `|amount|`, largest first.
    pub rows: Vec<CounterpartyBreakdownRow>,
    /// Everything beyond `limit` as one row (`counterpartyKey` null, named
    /// "Other"); `null` when nothing is beyond it. `rows + other` is the whole
    /// filtered set. Do not mistake it for the no-key bucket.
    pub other: Option<CounterpartyBreakdownRow>,
    pub currency: String,
}

/// Display name of the tail row.
pub const OTHER_NAME: &str = "Other";

#[derive(FromQueryResult)]
struct GroupRow {
    key: Option<String>,
    name: Option<String>,
    total: RustDecimal,
    count: i64,
}

#[derive(FromQueryResult)]
struct TotalRow {
    total: RustDecimal,
    count: i64,
    currency: Option<String>,
}

/// The tail bucket: whole filtered set minus the returned rows. `None` when no
/// transaction lies beyond them. Pure and unit-tested.
pub(crate) fn tail_row(
    total: RustDecimal,
    total_count: i64,
    rows: &[CounterpartyBreakdownRow],
) -> Option<CounterpartyBreakdownRow> {
    let shown_count: i64 = rows.iter().map(|r| i64::from(r.transaction_count)).sum();
    let beyond = total_count - shown_count;
    if beyond <= 0 {
        return None;
    }
    let shown: RustDecimal = rows.iter().map(|r| r.amount.0).sum();
    Some(CounterpartyBreakdownRow {
        counterparty_key: None,
        display_name: OTHER_NAME.to_string(),
        amount: Decimal(total - shown),
        transaction_count: i32::try_from(beyond).unwrap_or(i32::MAX),
    })
}

fn to_row(g: GroupRow, aliases: &AliasBook) -> CounterpartyBreakdownRow {
    let display_name = match g.key.as_deref() {
        None => UNGROUPED_NAME.to_string(),
        Some(k) => {
            let raw = g.name.unwrap_or_else(|| k.to_string());
            aliases.counterparty_display(Some(k), Some(&raw)).unwrap_or(raw)
        }
    };
    CounterpartyBreakdownRow {
        counterparty_key: g.key,
        display_name,
        amount: Decimal(g.total),
        transaction_count: i32::try_from(g.count).unwrap_or(i32::MAX),
    }
}

pub async fn fetch_counterparty_breakdown(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    aliases: &AliasBook,
    filter: &TransactionFilter,
    limit: i32,
) -> async_graphql::Result<CounterpartyBreakdown> {
    if scoped_ids.is_empty() {
        return Ok(CounterpartyBreakdown { rows: vec![], other: None, currency: "EUR".into() });
    }
    let condition = build_condition_with_category_filters(db, scoped_ids, filter).await?;
    let key = || Expr::cust("NULLIF(transaction.counterparty_key, '')");
    let net = || Expr::cust("SUM(transaction.amount)");

    let query = transaction::Entity::find()
        .filter(condition.clone())
        .select_only()
        .column_as(key(), "key")
        .column_as(Expr::cust("mode() WITHIN GROUP (ORDER BY transaction.counterparty_name)"), "name")
        .column_as(net(), "total")
        .column_as(Expr::cust("COUNT(*)"), "count")
        .group_by(key())
        .order_by(Expr::cust("ABS(SUM(transaction.amount))"), Order::Desc)
        .order_by(Expr::cust("NULLIF(transaction.counterparty_key, '')"), Order::Asc)
        .limit(clamp_limit(limit));
    tracing::debug!(sql = %sea_orm::QueryTrait::build(&query, sea_orm::DatabaseBackend::Postgres), "counterparty breakdown");
    let groups = query.into_model::<GroupRow>().all(db).await?;
    let rows: Vec<_> = groups.into_iter().map(|g| to_row(g, aliases)).collect();

    // Whole-set totals, to derive the tail (and the currency) without a
    // second pass over the groups.
    let totals = transaction::Entity::find()
        .filter(condition)
        .select_only()
        .column_as(Expr::cust("COALESCE(SUM(transaction.amount), 0)"), "total")
        .column_as(Expr::cust("COUNT(*)"), "count")
        .column_as(Expr::cust("mode() WITHIN GROUP (ORDER BY transaction.currency)"), "currency")
        .into_model::<TotalRow>()
        .one(db)
        .await?;
    let (other, currency) = match totals {
        Some(t) => (tail_row(t.total, t.count, &rows), t.currency.unwrap_or_else(|| "EUR".into())),
        None => (None, "EUR".into()),
    };
    Ok(CounterpartyBreakdown { rows, other, currency })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(amount: i64, count: i32) -> CounterpartyBreakdownRow {
        CounterpartyBreakdownRow {
            counterparty_key: Some("k".into()),
            display_name: "k".into(),
            amount: Decimal(RustDecimal::from(amount)),
            transaction_count: count,
        }
    }

    #[test]
    fn tail_is_the_whole_minus_the_shown_rows() {
        let rows = [row(-100, 2), row(30, 1)];
        let t = tail_row(RustDecimal::from(-60), 6, &rows).unwrap();
        assert_eq!(t.amount.0, RustDecimal::from(10));
        assert_eq!(t.transaction_count, 3);
        assert_eq!(t.counterparty_key, None);
        assert_eq!(t.display_name, OTHER_NAME);
    }

    #[test]
    fn no_tail_when_every_transaction_is_shown() {
        assert!(tail_row(RustDecimal::from(-70), 3, &[row(-100, 2), row(30, 1)]).is_none());
        assert!(tail_row(RustDecimal::ZERO, 0, &[]).is_none());
    }

    #[test]
    fn a_tail_that_nets_to_zero_is_still_reported() {
        // Two beyond-limit rows cancelling out are still transactions.
        let t = tail_row(RustDecimal::from(-100), 4, &[row(-100, 2)]).unwrap();
        assert_eq!(t.amount.0, RustDecimal::ZERO);
        assert_eq!(t.transaction_count, 2);
    }

    #[test]
    fn limit_is_clamped_not_rejected() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(-5), 1);
        assert_eq!(clamp_limit(i32::MAX), 200);
        assert_eq!(clamp_limit(20), 20);
    }
}
