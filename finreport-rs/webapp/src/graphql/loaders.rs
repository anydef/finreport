//! Batched per-account lookups (§9 WP4): `accounts` resolves `latestBalance`
//! for every scoped account in one query rather than one-per-account.
//!
//! This is the N+1-avoidance the spec calls for. It is **not** wired through
//! `async_graphql::dataloader::DataLoader` — that type needs the
//! `dataloader` + `boxed-trait` features on the `async-graphql` dependency,
//! and `webapp/Cargo.toml` is WP0-owned (§9 shared-file protocol); see the
//! WP4 handoff notes for the one-line feature addition that would let this
//! become a real per-request-batched `DataLoader` instead of this
//! prefetch-up-front version, which already avoids the N+1 query pattern
//! for every call this iteration makes (`accounts` is the only place it
//! could occur: nothing nests `Account` under a per-row resolver).

use entity::entities::account_balance;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, Order, QueryFilter, QueryOrder};
use std::collections::HashMap;
use uuid::Uuid;

use crate::graphql::types::Balance;

/// Fetches the most recent `account_balance` row (by `balance_date`) for
/// each of `account_ids`, in one query.
pub async fn latest_balances(
    db: &DatabaseConnection,
    account_ids: &[Uuid],
) -> Result<HashMap<Uuid, Balance>, sea_orm::DbErr> {
    if account_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = account_balance::Entity::find()
        .filter(account_balance::Column::AccountId.is_in(account_ids.to_vec()))
        .order_by(account_balance::Column::AccountId, Order::Asc)
        .order_by(account_balance::Column::BalanceDate, Order::Desc)
        .all(db)
        .await?;

    let mut out = HashMap::new();
    for row in rows {
        // Rows arrive ordered newest-first within each account_id run; the
        // first one seen per key is the latest balance.
        out.entry(row.account_id).or_insert_with(|| Balance {
            date: row.balance_date.into(),
            amount: row.amount.into(),
            currency: row.currency,
        });
    }
    Ok(out)
}
