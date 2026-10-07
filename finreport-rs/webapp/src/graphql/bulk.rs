//! `setTransactionsCategory` / `setTransactionsTags`: apply one decision to
//! every transaction a `TransactionFilter` matches.
//!
//! The mutations take a *filter*, not an id list, so "all 300 matching"
//! means all 300 rather than the page the client happened to load. Each
//! matched transaction is then handled exactly as its single-transaction
//! counterpart would handle it, by calling the same cores
//! (`labels::apply_category`, `insights::apply_tags`): one whole-state
//! `UserLabelRecord` published to `finreport.user-label`, then the guarded
//! projection upsert. There is no second write path here.
//!
//! Decisions worth knowing about:
//!
//! - **Scoping.** The filter is always ANDed with the caller's own accounts
//!   (`scoped_account_ids`), so a `transactionIds` entry belonging to someone
//!   else simply matches nothing, indistinguishable from an id that does not
//!   exist.
//! - **Empty filter is allowed.** It matches every transaction the caller
//!   owns. The UI only sends it for an explicit "all matching" selection,
//!   the caller can only ever reach their own rows, and the match-size cap
//!   below bounds the blast radius. Refusing it would just push users to
//!   invent a dummy date range.
//! - **Match-size cap.** `Settings::bulk_edit_max_transactions` (default
//!   5000). A larger match set is rejected up front with
//!   `BULK_LIMIT_EXCEEDED` and *nothing* is changed; silently truncating
//!   would leave "all matching" half-applied with no way to tell which half.
//! - **Not atomic, and honest about it.** A failing row is logged with its
//!   id, counted in `failed`, and the rest carry on. Rows already in the
//!   requested state are skipped (counted in `matched` only), so repeating
//!   a call is cheap and reports `applied = 0` once everything has landed.

use async_graphql::{ErrorExtensions, SimpleObject};
use entity::entities::{transaction, transaction_split, transaction_tag, transaction_user_label};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use uuid::Uuid;

use crate::graphql::categories::find_by_slug;
use crate::graphql::current_user::{scoped_account_ids, AuthenticatedUser};
use crate::graphql::events::kafka_unavailable_error;
use crate::graphql::transactions::build_condition_with_category_filters;
use crate::graphql::types::TransactionFilter;
use crate::graphql::{insights, labels};
use crate::kafka::producer::EventPublisher;

/// Outcome of a bulk edit. Always `matched >= applied + failed`; the
/// remainder were already in the requested state.
#[derive(SimpleObject, Debug, Default, PartialEq, Eq)]
pub struct BulkEditResult {
    /// How many transactions the filter matched.
    pub matched: i32,
    /// How many were actually changed.
    pub applied: i32,
    /// How many failed; the operation is not atomic.
    pub failed: i32,
    /// How many had splits that this category change discarded.
    pub splits_cleared: i32,
}

/// What happened to one matched transaction.
#[derive(Debug, PartialEq, Eq)]
enum RowOutcome {
    /// Written. `splits_cleared` is the number of split rows discarded.
    Applied { splits_cleared: u64 },
    /// Already in the requested state, or a newer revision won; untouched.
    Unchanged,
}

fn validation_error(message: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(message.into()).extend_with(|_, e| e.set("code", "VALIDATION"))
}

fn limit_exceeded_error(limit: u32) -> async_graphql::Error {
    async_graphql::Error::new(format!(
        "the filter matches more than {limit} transactions; narrow it (APP_bulk_edit_max_transactions) — nothing was changed"
    ))
    .extend_with(|_, e| e.set("code", "BULK_LIMIT_EXCEEDED"))
}

/// Every transaction the caller may edit that `filter` matches, oldest
/// first. This is the single filter-to-rows query both mutations use.
///
/// Fails with `BULK_LIMIT_EXCEEDED` when more than `max` rows match.
async fn matching_transactions(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    filter: &TransactionFilter,
    max: u32,
) -> async_graphql::Result<Vec<transaction::Model>> {
    let requested: Option<Vec<Uuid>> = filter
        .account_ids
        .as_ref()
        .map(|ids| ids.iter().map(|id| id.0).collect());
    let scoped_ids = scoped_account_ids(user, requested.as_deref())?;
    if scoped_ids.is_empty() {
        return Ok(Vec::new());
    }
    let condition = build_condition_with_category_filters(db, &scoped_ids, filter).await?;
    let rows = transaction::Entity::find()
        .filter(condition)
        .order_by_asc(transaction::Column::BookingDate)
        .order_by_asc(transaction::Column::Id)
        // One past the cap is enough to know it was exceeded.
        .limit(u64::from(max) + 1)
        .all(db)
        .await?;
    if rows.len() as u64 > u64::from(max) {
        tracing::warn!(
            user_id = %user.user_id,
            limit = max,
            "bulk edit rejected: filter matches more than the configured maximum"
        );
        return Err(limit_exceeded_error(max));
    }
    Ok(rows)
}

/// Runs `apply` over every row, never aborting on a failure: an `Err` is
/// logged with the transaction id and counted in `failed`.
async fn run_bulk<F, Fut>(rows: Vec<transaction::Model>, mut apply: F) -> BulkEditResult
where
    F: FnMut(transaction::Model) -> Fut,
    Fut: Future<Output = async_graphql::Result<RowOutcome>>,
{
    let mut result = BulkEditResult {
        matched: rows.len() as i32,
        ..Default::default()
    };
    for row in rows {
        let id = row.id;
        match apply(row).await {
            Ok(RowOutcome::Applied { splits_cleared }) => {
                result.applied += 1;
                result.splits_cleared += splits_cleared as i32;
            }
            Ok(RowOutcome::Unchanged) => {}
            Err(error) => {
                result.failed += 1;
                tracing::error!(transaction_id = %id, error = %error.message, "bulk edit failed for transaction");
            }
        }
    }
    result
}

/// Transactions among `ids` that are already pinned to `category_id` with
/// no splits in the way: a category write would change nothing.
async fn already_in_category(
    db: &DatabaseConnection,
    ids: &[Uuid],
    category_id: Uuid,
) -> async_graphql::Result<HashSet<Uuid>> {
    let pinned: HashSet<Uuid> = transaction_user_label::Entity::find()
        .filter(transaction_user_label::Column::TransactionId.is_in(ids.to_vec()))
        .filter(transaction_user_label::Column::CategoryId.eq(category_id))
        .all(db)
        .await?
        .into_iter()
        .map(|r| r.transaction_id)
        .collect();
    let split: HashSet<Uuid> = transaction_split::Entity::find()
        .filter(transaction_split::Column::TransactionId.is_in(ids.to_vec()))
        .all(db)
        .await?
        .into_iter()
        .map(|r| r.transaction_id)
        .collect();
    Ok(pinned.difference(&split).copied().collect())
}

/// Current tag set (sorted) of every transaction among `ids` that has one.
async fn current_tags(
    db: &DatabaseConnection,
    ids: &[Uuid],
) -> async_graphql::Result<HashMap<Uuid, Vec<String>>> {
    let mut by_id: HashMap<Uuid, Vec<String>> = HashMap::new();
    for row in transaction_tag::Entity::find()
        .filter(transaction_tag::Column::TransactionId.is_in(ids.to_vec()))
        .all(db)
        .await?
    {
        by_id.entry(row.transaction_id).or_default().push(row.tag);
    }
    for tags in by_id.values_mut() {
        tags.sort();
    }
    Ok(by_id)
}

/// `setTransactionsCategory`: override the category of every matched
/// transaction, clearing its splits (as the single mutation does).
pub async fn set_transactions_category(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    filter: &TransactionFilter,
    category_slug: &str,
    max: u32,
) -> async_graphql::Result<BulkEditResult> {
    // Validate everything before touching anything.
    let category = find_by_slug(db, category_slug)
        .await?
        .ok_or_else(|| validation_error(format!("category '{category_slug}' does not exist")))?;
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    let rows = matching_transactions(db, user, filter, max).await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let done = already_in_category(db, &ids, category.id).await?;

    let result = run_bulk(rows, |txn| {
        let category = &category;
        let skip = done.contains(&txn.id);
        async move {
            if skip {
                return Ok(RowOutcome::Unchanged);
            }
            Ok(match labels::apply_category(db, publisher, &txn, category).await? {
                Some(splits_cleared) => RowOutcome::Applied { splits_cleared },
                None => RowOutcome::Unchanged,
            })
        }
    })
    .await;
    tracing::info!(user_id = %user.user_id, category = category_slug, ?result, "bulk category change finished");
    Ok(result)
}

/// `setTransactionsTags`: replace the tag set of every matched transaction
/// (`[]` clears), preserving each one's category and recurring override.
pub async fn set_transactions_tags(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    filter: &TransactionFilter,
    tags: &[String],
    max_tags: u32,
    max: u32,
) -> async_graphql::Result<BulkEditResult> {
    let normalized = insights::normalize_tags(tags, max_tags)?;
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    let rows = matching_transactions(db, user, filter, max).await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let current = current_tags(db, &ids).await?;

    let result = run_bulk(rows, |txn| {
        let normalized = &normalized;
        let skip = current.get(&txn.id).map(Vec::as_slice).unwrap_or(&[]) == normalized.as_slice();
        async move {
            if skip {
                return Ok(RowOutcome::Unchanged);
            }
            Ok(if insights::apply_tags(db, publisher, &txn, normalized).await? {
                RowOutcome::Applied { splits_cleared: 0 }
            } else {
                RowOutcome::Unchanged
            })
        }
    })
    .await;
    tracing::info!(user_id = %user.user_id, ?result, "bulk tag change finished");
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn row() -> transaction::Model {
        transaction::Model {
            id: Uuid::new_v4(),
            account_id: Uuid::new_v4(),
            source: "test".into(),
            external_id: "x".into(),
            booking_date: chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            valuta_date: None,
            booking_status: "BOOKED".into(),
            amount: rust_decimal::Decimal::ONE,
            currency: "EUR".into(),
            counterparty_name: None,
            counterparty_iban: None,
            description: None,
            transaction_type: None,
            raw_payload: serde_json::json!({}),
            origin: "test".into(),
            imported_at: Utc::now().into(),
            updated_at: Utc::now().into(),
            counterparty_key: None,
        }
    }

    #[tokio::test]
    async fn one_failing_row_does_not_stop_the_rest_and_is_counted() {
        let rows: Vec<_> = (0..4).map(|_| row()).collect();
        let bad = rows[1].id;
        let result = run_bulk(rows, |txn| {
            let fail = txn.id == bad;
            async move {
                if fail {
                    Err(async_graphql::Error::new("boom"))
                } else {
                    Ok(RowOutcome::Applied { splits_cleared: 2 })
                }
            }
        })
        .await;
        assert_eq!(
            result,
            BulkEditResult { matched: 4, applied: 3, failed: 1, splits_cleared: 6 }
        );
    }

    #[tokio::test]
    async fn unchanged_rows_count_as_matched_only() {
        let rows: Vec<_> = (0..3).map(|_| row()).collect();
        let result = run_bulk(rows, |_| async { Ok(RowOutcome::Unchanged) }).await;
        assert_eq!(
            result,
            BulkEditResult { matched: 3, applied: 0, failed: 0, splits_cleared: 0 }
        );
    }

    #[tokio::test]
    async fn no_rows_is_an_empty_result() {
        let result = run_bulk(Vec::new(), |_| async { Ok(RowOutcome::Unchanged) }).await;
        assert_eq!(result, BulkEditResult::default());
    }

    #[test]
    fn limit_error_carries_a_stable_code() {
        let err = limit_exceeded_error(5000);
        assert_eq!(
            err.extensions.unwrap().get("code"),
            Some(&async_graphql::Value::String("BULK_LIMIT_EXCEEDED".into()))
        );
    }
}
