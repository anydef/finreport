//! `categoryComparison`: per-category and total spending across consecutive
//! calendar periods, so a trend and "which categories drove it" come from one
//! query.
//!
//! Nothing here aggregates transactions. Each period is a call to
//! [`breakdown::fetch_breakdown`] — so splits, transfers, uncategorised and
//! held rows, the signed-net-then-magnitude rule and the caller's account
//! scoping behave exactly as they do on the dashboard's "Spending by
//! category" card, and the two can never disagree. The periods themselves come
//! from `cashflow::summary::fill_dense_buckets`, the same enumeration
//! `cashflowSummary` uses. This module only pivots the per-period breakdowns
//! into a (category x period) matrix ([`assemble`], pure and unit-tested).

use chrono::NaiveDate;
use rust_decimal::Decimal as RustDecimal;
use sea_orm::DatabaseConnection;
use std::collections::HashMap;
use uuid::Uuid;

use crate::graphql::breakdown::fetch_breakdown;
use crate::graphql::cashflow::summary::fill_dense_buckets;
use crate::graphql::cashflow::{bounded_range, first_currency};
use crate::graphql::scalars::{Date as GqlDate, Decimal as GqlDecimal};
use crate::graphql::types::{
    Category, CategoryBreakdown, CategoryKind, Granularity, TransactionFilter,
};
use async_graphql::{ErrorExtensions, SimpleObject};

/// More periods than this is a chart nobody can read and N breakdown passes
/// the server would pay for; ask for a coarser granularity instead.
pub const MAX_PERIODS: usize = 60;

#[derive(SimpleObject)]
pub struct ComparisonPeriod {
    /// Inclusive. The first and last periods are clipped to the requested range.
    pub start: GqlDate,
    /// Inclusive.
    pub end: GqlDate,
    /// Everything spent in the period that is not held for review: the
    /// categories' amounts plus `uncategorized`. This is the headline figure.
    pub total: GqlDecimal,
    /// Unlabelled spending, as the breakdown's `uncategorized` row (magnitude of
    /// the signed net). Included in `total`.
    pub uncategorized: GqlDecimal,
    /// Held for review, as the breakdown's `needsReview` row. Deliberately NOT
    /// in `total` and in no category: the label is not trusted yet.
    pub needs_review: GqlDecimal,
}

#[derive(SimpleObject)]
pub struct ComparisonCell {
    /// Magnitude of the category's signed net in the period; `0` when the
    /// category had no transactions (or a refund cancelled them).
    pub amount: GqlDecimal,
    /// `0` means the category was absent from the period altogether, as opposed
    /// to present and netting to zero.
    pub transaction_count: i32,
}

#[derive(SimpleObject)]
pub struct ComparisonCategory {
    pub category: Category,
    /// Sum of `cells`, so the list can be ordered by overall weight.
    pub total: GqlDecimal,
    /// One per entry of `periods`, in the same order.
    pub cells: Vec<ComparisonCell>,
}

#[derive(SimpleObject)]
pub struct CategoryComparison {
    pub periods: Vec<ComparisonPeriod>,
    /// Every category that appears in at least one period, heaviest first.
    pub categories: Vec<ComparisonCategory>,
    pub currency: String,
}

/// Pivots one breakdown per period into the comparison matrix.
pub fn assemble(
    periods: &[(NaiveDate, NaiveDate)],
    breakdowns: Vec<CategoryBreakdown>,
    currency: String,
) -> CategoryComparison {
    debug_assert_eq!(periods.len(), breakdowns.len());

    let mut by_category: HashMap<Uuid, (Category, Vec<(RustDecimal, i32)>)> = HashMap::new();
    let mut period_rows = Vec::with_capacity(periods.len());

    for (index, (&(start, end), breakdown)) in periods.iter().zip(breakdowns).enumerate() {
        let mut categorised = RustDecimal::ZERO;
        for row in breakdown.rows {
            categorised += row.amount.0;
            let entry = by_category
                .entry(row.category.id.0)
                .or_insert_with(|| (row.category.clone(), vec![(RustDecimal::ZERO, 0); periods.len()]));
            entry.1[index] = (row.amount.0, row.transaction_count);
        }
        let uncategorized = breakdown.uncategorized.map_or(RustDecimal::ZERO, |r| r.amount.0);
        let needs_review = breakdown.needs_review.map_or(RustDecimal::ZERO, |r| r.amount.0);
        period_rows.push(ComparisonPeriod {
            start: GqlDate(start),
            end: GqlDate(end),
            total: GqlDecimal(categorised + uncategorized),
            uncategorized: GqlDecimal(uncategorized),
            needs_review: GqlDecimal(needs_review),
        });
    }

    let mut categories: Vec<ComparisonCategory> = by_category
        .into_values()
        .map(|(category, cells)| ComparisonCategory {
            category,
            total: GqlDecimal(cells.iter().map(|(amount, _)| *amount).sum()),
            cells: cells
                .into_iter()
                .map(|(amount, count)| ComparisonCell {
                    amount: GqlDecimal(amount),
                    transaction_count: count,
                })
                .collect(),
        })
        .collect();
    // Heaviest first; the slug breaks ties so the order is deterministic.
    categories.sort_by(|a, b| {
        b.total.0.cmp(&a.total.0).then_with(|| a.category.slug.cmp(&b.category.slug))
    });

    CategoryComparison { periods: period_rows, categories, currency }
}

pub async fn fetch_comparison(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    mut filter: TransactionFilter,
    granularity: Granularity,
    level: i32,
    kind: CategoryKind,
) -> async_graphql::Result<CategoryComparison> {
    let (range_start, range_end) = bounded_range(&filter)?;
    let periods: Vec<(NaiveDate, NaiveDate)> =
        fill_dense_buckets(&[], range_start, range_end, granularity)
            .into_iter()
            .map(|b| (b.start, b.end))
            .collect();
    if periods.len() > MAX_PERIODS {
        return Err(async_graphql::Error::new(format!(
            "categoryComparison: {} periods requested, at most {MAX_PERIODS} - use a coarser granularity",
            periods.len()
        ))
        .extend_with(|_, e| e.set("code", "VALIDATION")));
    }

    let mut breakdowns = Vec::with_capacity(periods.len());
    for &(start, end) in &periods {
        // The caller's other predicates (accounts, search, tags, ...) carry
        // through unchanged; only the window moves.
        filter.start_date = Some(GqlDate(start));
        filter.end_date = Some(GqlDate(end));
        breakdowns.push(fetch_breakdown(db, scoped_ids, &filter, level, Some(kind)).await?);
    }

    let currency = first_currency(db, scoped_ids).await?;
    Ok(assemble(&periods, breakdowns, currency))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::scalars::Uuid as GqlUuid;
    use crate::graphql::types::{CategoryBreakdownRow, CategoryKind as Kind};
    use std::str::FromStr;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn dec(s: &str) -> RustDecimal {
        RustDecimal::from_str(s).unwrap()
    }

    fn category(slug: &str) -> Category {
        Category {
            id: GqlUuid(Uuid::new_v5(&Uuid::NAMESPACE_OID, slug.as_bytes())),
            slug: slug.to_string(),
            name: slug.to_string(),
            kind: Kind::Expense,
            parent_id: None,
            depth: 1,
            archived: false,
            origin: "seed".to_string(),
        }
    }

    fn row(slug: &str, amount: &str, count: i32) -> CategoryBreakdownRow {
        CategoryBreakdownRow {
            category: category(slug),
            amount: GqlDecimal(dec(amount)),
            transaction_count: count,
            share: 0.0,
        }
    }

    fn breakdown(rows: Vec<CategoryBreakdownRow>) -> CategoryBreakdown {
        CategoryBreakdown { rows, uncategorized: None, needs_review: None, currency: "EUR".into() }
    }

    fn two_months() -> Vec<(NaiveDate, NaiveDate)> {
        vec![(d(2026, 8, 1), d(2026, 8, 31)), (d(2026, 9, 1), d(2026, 9, 30))]
    }

    #[test]
    fn aligns_every_category_to_every_period_with_zero_cells_where_absent() {
        let out = assemble(
            &two_months(),
            vec![
                breakdown(vec![row("food", "100", 4), row("gym", "30", 1)]),
                breakdown(vec![row("food", "120", 5), row("travel", "400", 2)]),
            ],
            "EUR".into(),
        );
        assert_eq!(out.categories.len(), 3);
        for c in &out.categories {
            assert_eq!(c.cells.len(), 2, "{}", c.category.slug);
        }
        let by = |slug: &str| out.categories.iter().find(|c| c.category.slug == slug).unwrap();
        // gym disappears, travel appears
        assert_eq!(by("gym").cells[1].amount.0, RustDecimal::ZERO);
        assert_eq!(by("gym").cells[1].transaction_count, 0);
        assert_eq!(by("travel").cells[0].transaction_count, 0);
        assert_eq!(by("travel").cells[1].amount.0, dec("400"));
        assert_eq!(by("food").total.0, dec("220"));
    }

    #[test]
    fn orders_categories_heaviest_first_with_slug_tiebreak() {
        let out = assemble(
            &two_months()[..1],
            vec![breakdown(vec![row("b", "10", 1), row("a", "10", 1), row("c", "50", 1)])],
            "EUR".into(),
        );
        let slugs: Vec<_> = out.categories.iter().map(|c| c.category.slug.as_str()).collect();
        assert_eq!(slugs, ["c", "a", "b"]);
    }

    #[test]
    fn period_total_is_categories_plus_uncategorized_and_never_held() {
        let mut b = breakdown(vec![row("food", "100", 4)]);
        b.uncategorized = Some(row("uncategorized", "25", 2));
        b.needs_review = Some(row("needs_review", "70", 3));
        let out = assemble(&two_months()[..1], vec![b], "EUR".into());
        let p = &out.periods[0];
        assert_eq!(p.total.0, dec("125"));
        assert_eq!(p.uncategorized.0, dec("25"));
        assert_eq!(p.needs_review.0, dec("70"));
        assert_eq!(out.categories.len(), 1, "the sentinels are not categories");
    }

    #[test]
    fn an_empty_period_is_zero_not_missing() {
        let out = assemble(
            &two_months(),
            vec![breakdown(vec![]), breakdown(vec![row("food", "5", 1)])],
            "EUR".into(),
        );
        assert_eq!(out.periods.len(), 2);
        assert_eq!(out.periods[0].total.0, RustDecimal::ZERO);
        assert_eq!(out.periods[1].total.0, dec("5"));
    }

    #[test]
    fn no_periods_yields_an_empty_comparison() {
        let out = assemble(&[], vec![], "EUR".into());
        assert!(out.periods.is_empty() && out.categories.is_empty());
    }
}
