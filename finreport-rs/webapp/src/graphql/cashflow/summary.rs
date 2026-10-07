//! `cashflowSummary` (§5): SQL does the aggregation (`date_trunc` + `SUM(…)
//! FILTER (WHERE …)`), Rust only dense-fills the empty buckets — pure and
//! unit-tested, no DB needed (§8).

use chrono::{Datelike, Duration, NaiveDate};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::graphql::types::{Direction, Granularity, TransactionFilter};

/// One Postgres-aggregated row: a bucket that had at least one transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct SparseBucket {
    pub bucket_start: NaiveDate,
    pub income: Decimal,
    pub spending: Decimal,
    pub transaction_count: i64,
}

/// A fully dense bucket — every period in the requested range, zero-filled
/// where Postgres returned nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct DenseBucket {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub income: Decimal,
    pub spending: Decimal,
    pub net: Decimal,
    pub transaction_count: i64,
}

/// `date_trunc` alignment matching the SQL side (§5): `WEEK` starts Monday
/// (ISO), `MONTH` buckets are calendar months.
fn align_down(date: NaiveDate, granularity: Granularity) -> NaiveDate {
    match granularity {
        Granularity::Day => date,
        Granularity::Week => date - Duration::days(date.weekday().num_days_from_monday() as i64),
        Granularity::Month => NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap(),
    }
}

/// The bucket's own natural end, before clipping to the requested range.
fn natural_end(bucket_start: NaiveDate, granularity: Granularity) -> NaiveDate {
    match granularity {
        Granularity::Day => bucket_start,
        Granularity::Week => bucket_start + Duration::days(6),
        Granularity::Month => {
            let next_month = if bucket_start.month() == 12 {
                NaiveDate::from_ymd_opt(bucket_start.year() + 1, 1, 1).unwrap()
            } else {
                NaiveDate::from_ymd_opt(bucket_start.year(), bucket_start.month() + 1, 1).unwrap()
            };
            next_month - Duration::days(1)
        }
    }
}

fn next_bucket_start(bucket_start: NaiveDate, granularity: Granularity) -> NaiveDate {
    match granularity {
        Granularity::Day => bucket_start + Duration::days(1),
        Granularity::Week => bucket_start + Duration::days(7),
        Granularity::Month => {
            if bucket_start.month() == 12 {
                NaiveDate::from_ymd_opt(bucket_start.year() + 1, 1, 1).unwrap()
            } else {
                NaiveDate::from_ymd_opt(bucket_start.year(), bucket_start.month() + 1, 1).unwrap()
            }
        }
    }
}

/// Dense-fills `[range_start, range_end]` (both inclusive) per `granularity`,
/// zero-filling periods `sparse` has no row for. Buckets at either end are
/// clipped to the requested range (§5) even though they align to calendar
/// weeks/months internally.
pub fn fill_dense_buckets(
    sparse: &[SparseBucket],
    range_start: NaiveDate,
    range_end: NaiveDate,
    granularity: Granularity,
) -> Vec<DenseBucket> {
    if range_start > range_end {
        return Vec::new();
    }

    let by_key: std::collections::HashMap<NaiveDate, &SparseBucket> =
        sparse.iter().map(|b| (b.bucket_start, b)).collect();

    let mut buckets = Vec::new();
    let mut key = align_down(range_start, granularity);
    while key <= range_end {
        let end = natural_end(key, granularity).min(range_end);
        let start = key.max(range_start);
        let (income, spending, count) = by_key
            .get(&key)
            .map(|b| (b.income, b.spending, b.transaction_count))
            .unwrap_or((Decimal::ZERO, Decimal::ZERO, 0));
        buckets.push(DenseBucket {
            start,
            end,
            income,
            spending,
            net: income - spending,
            transaction_count: count,
        });
        key = next_bucket_start(key, granularity);
    }
    buckets
}

/// Builds the SQL (text + positional params, in order) for the sparse
/// aggregation query, sharing `TransactionFilter`'s search/direction/
/// counterparty predicates with the plain `transactions` list query.
///
/// Returns `(sql, params)`; params are, in positional order: granularity
/// text, the scoped account id array, range start, range end, then one
/// entry per optional predicate actually appended.
pub fn build_summary_sql(
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
    range_start: NaiveDate,
    range_end: NaiveDate,
    granularity: Granularity,
) -> (String, Vec<sea_orm::Value>) {
    let granularity_sql = match granularity {
        Granularity::Day => "day",
        Granularity::Week => "week",
        Granularity::Month => "month",
    };

    let mut sql = String::from(
        "SELECT date_trunc($1, booking_date::timestamp)::date AS bucket_start, \
         COALESCE(SUM(amount) FILTER (WHERE amount > 0), 0) AS income, \
         COALESCE(SUM(-amount) FILTER (WHERE amount < 0), 0) AS spending, \
         COUNT(*) AS tx_count \
         FROM transaction \
         WHERE account_id = ANY($2) AND booking_date >= $3 AND booking_date <= $4",
    );
    let mut params: Vec<sea_orm::Value> = vec![
        granularity_sql.into(),
        scoped_ids.to_vec().into(),
        range_start.into(),
        range_end.into(),
    ];
    let mut idx = 5;

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
    let (id_amount_sql, id_amount_params) =
        crate::graphql::transactions::id_and_amount_sql("transaction", filter, idx);
    sql.push_str(&id_amount_sql);
    params.extend(id_amount_params);
    match filter.has_counterparty {
        Some(true) => sql.push_str(" AND counterparty_name IS NOT NULL"),
        Some(false) => sql.push_str(" AND counterparty_name IS NULL"),
        None => {}
    }
    // §4 "totals exclude transfers" (unless the caller explicitly asked for
    // them via `filter.transfer = true`).
    sql.push_str(&crate::graphql::cashflow::transfer_filter_sql("id", filter));

    sql.push_str(" GROUP BY bucket_start ORDER BY bucket_start");
    (sql, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn empty_range_produces_no_buckets() {
        let buckets = fill_dense_buckets(&[], date(2024, 2, 2), date(2024, 2, 1), Granularity::Day);
        assert!(buckets.is_empty());
    }

    #[test]
    fn single_day_range_is_dense_even_with_no_data() {
        let buckets = fill_dense_buckets(&[], date(2024, 6, 15), date(2024, 6, 15), Granularity::Day);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].start, date(2024, 6, 15));
        assert_eq!(buckets[0].end, date(2024, 6, 15));
        assert_eq!(buckets[0].income, Decimal::ZERO);
        assert_eq!(buckets[0].transaction_count, 0);
    }

    #[test]
    fn day_granularity_fills_gaps_between_sparse_rows() {
        let sparse = vec![
            SparseBucket {
                bucket_start: date(2024, 1, 1),
                income: Decimal::new(1000, 2),
                spending: Decimal::ZERO,
                transaction_count: 1,
            },
            SparseBucket {
                bucket_start: date(2024, 1, 3),
                income: Decimal::ZERO,
                spending: Decimal::new(500, 2),
                transaction_count: 1,
            },
        ];
        let buckets = fill_dense_buckets(&sparse, date(2024, 1, 1), date(2024, 1, 3), Granularity::Day);
        assert_eq!(buckets.len(), 3);
        assert_eq!(buckets[1].income, Decimal::ZERO);
        assert_eq!(buckets[1].transaction_count, 0);
        assert_eq!(buckets[2].spending, Decimal::new(500, 2));
    }

    #[test]
    fn week_buckets_align_to_iso_monday() {
        // 2024-06-15 is a Saturday; its ISO week starts Monday 2024-06-10.
        let buckets = fill_dense_buckets(&[], date(2024, 6, 10), date(2024, 6, 16), Granularity::Week);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].start, date(2024, 6, 10));
        assert_eq!(buckets[0].end, date(2024, 6, 16));
    }

    #[test]
    fn week_boundary_spanning_two_weeks_yields_two_buckets() {
        let buckets = fill_dense_buckets(&[], date(2024, 6, 9), date(2024, 6, 10), Granularity::Week);
        // 2024-06-09 is a Sunday (ISO week 23, Mon 2024-06-03..Sun 2024-06-09);
        // 2024-06-10 is the Monday starting the next ISO week.
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].start, date(2024, 6, 9));
        assert_eq!(buckets[0].end, date(2024, 6, 9));
        assert_eq!(buckets[1].start, date(2024, 6, 10));
        assert_eq!(buckets[1].end, date(2024, 6, 10));
    }

    #[test]
    fn month_bucket_is_clipped_at_both_ends() {
        let buckets = fill_dense_buckets(&[], date(2024, 2, 10), date(2024, 2, 20), Granularity::Month);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].start, date(2024, 2, 10));
        assert_eq!(buckets[0].end, date(2024, 2, 20));
    }

    #[test]
    fn month_range_spanning_a_year_boundary() {
        let buckets = fill_dense_buckets(&[], date(2023, 12, 15), date(2024, 1, 15), Granularity::Month);
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].start, date(2023, 12, 15));
        assert_eq!(buckets[0].end, date(2023, 12, 31));
        assert_eq!(buckets[1].start, date(2024, 1, 1));
        assert_eq!(buckets[1].end, date(2024, 1, 15));
    }

    #[test]
    fn net_is_income_minus_spending() {
        let sparse = vec![SparseBucket {
            bucket_start: date(2024, 1, 1),
            income: Decimal::new(10000, 2),
            spending: Decimal::new(4000, 2),
            transaction_count: 2,
        }];
        let buckets = fill_dense_buckets(&sparse, date(2024, 1, 1), date(2024, 1, 1), Granularity::Day);
        assert_eq!(buckets[0].net, Decimal::new(6000, 2));
    }
}
