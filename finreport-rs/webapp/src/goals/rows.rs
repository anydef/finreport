//! The one place goal evaluation touches SQL: the contribution-row query
//! (iteration 4 §3.1). Everything downstream is a pure function over its
//! result.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

use super::counting::ContributionRow;
use crate::graphql::cashflow::transfer_exclusion_sql;

/// Contribution rows for `[start, end]` over the scoped accounts: an unsplit
/// transaction yields one row (its own amount and resolved category), a
/// transaction with valid splits yields one per valid part. Internal
/// transfers are dropped with the cashflow queries' own predicate.
///
/// A transaction's category is `transaction_label.category_id` whatever the
/// label's status: a NULL one means no category (consistent with the
/// `uncategorized` filter), and a transaction with valid splits is
/// categorised through its parts.
pub async fn fetch_rows(
    db: &DatabaseConnection,
    scoped_account_ids: &[Uuid],
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<ContributionRow>, DbErr> {
    if scoped_account_ids.is_empty() || start > end {
        return Ok(Vec::new());
    }
    let sql = build_rows_sql();
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        sql,
        vec![scoped_account_ids.to_vec().into(), start.into(), end.into()],
    );
    db.query_all(stmt)
        .await?
        .iter()
        .map(|row| {
            Ok(ContributionRow {
                transaction_id: row.try_get("", "transaction_id")?,
                part_index: row.try_get("", "part_index")?,
                amount: row.try_get::<Decimal>("", "amount")?,
                category_slug: row.try_get("", "category_slug")?,
                category_kind: row.try_get("", "category_kind")?,
                tags: row.try_get("", "tags")?,
                booking_date: row.try_get("", "booking_date")?,
                held_for_review: row.try_get("", "held")?,
            })
        })
        .collect()
}

fn build_rows_sql() -> String {
    let excl = transfer_exclusion_sql("t.id");
    let tags = "COALESCE((SELECT array_agg(tt.tag ORDER BY tt.tag) FROM transaction_tag tt \
                WHERE tt.transaction_id = t.id), ARRAY[]::text[])";
    let held = "COALESCE(tl.status = 'needs_review', false)";
    format!(
        "SELECT t.id AS transaction_id, NULL::int4 AS part_index, t.amount AS amount, \
           c.slug AS category_slug, c.kind AS category_kind, {tags} AS tags, \
           t.booking_date AS booking_date, {held} AS held \
         FROM transaction t \
         LEFT JOIN transaction_label tl ON tl.transaction_id = t.id \
         LEFT JOIN category c ON c.id = tl.category_id \
         WHERE t.account_id = ANY($1) AND t.booking_date >= $2 AND t.booking_date <= $3 \
           AND NOT EXISTS (SELECT 1 FROM transaction_split ts \
                           WHERE ts.transaction_id = t.id AND ts.invalid = false) \
           {excl} \
         UNION ALL \
         SELECT t.id AS transaction_id, ts.part_index AS part_index, ts.amount AS amount, \
           c.slug AS category_slug, c.kind AS category_kind, {tags} AS tags, \
           t.booking_date AS booking_date, {held} AS held \
         FROM transaction_split ts \
         JOIN transaction t ON t.id = ts.transaction_id \
         LEFT JOIN transaction_label tl ON tl.transaction_id = t.id \
         LEFT JOIN category c ON c.id = ts.category_id \
         WHERE ts.invalid = false AND t.account_id = ANY($1) \
           AND t.booking_date >= $2 AND t.booking_date <= $3 \
           {excl}"
    )
}
