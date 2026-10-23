//! Savings accounts: which accounts the caller treats as savings
//! (`Account.isSavings`, `setAccountSavings`).
//!
//! The flag is **per user** - a reporting choice, not a fact from the bank -
//! so it lives on its own compacted topic keyed by user + account
//! (`kafka::savings`), projected into `account_savings`. The read side that
//! uses it (`savingsSummary`) is a separate slice.

use async_graphql::Context;
use chrono::Utc;
use entity::entities::account_savings;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

use crate::graphql::accounts;
use crate::graphql::current_user::current_user;
use crate::graphql::events::{kafka_unavailable_error, publish_event, publish_tombstone};
use crate::graphql::types::Account;
use crate::kafka::producer::EventPublisher;
use crate::kafka::savings::{
    account_savings_topic_key, AccountSavingsRecord, CURRENT_SCHEMA_VERSION, TOPIC_ACCOUNT_SAVINGS,
};
use crate::projection::account_savings as proj;

/// The caller's flagged accounts, restricted to ones they can still see.
pub async fn flagged_accounts(
    db: &DatabaseConnection,
    user_id: Uuid,
    scoped_ids: &[Uuid],
) -> Result<HashSet<Uuid>, sea_orm::DbErr> {
    if scoped_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let rows = account_savings::Entity::find()
        .filter(account_savings::Column::UserId.eq(user_id))
        .filter(account_savings::Column::AccountId.is_in(scoped_ids.to_vec()))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| r.account_id).collect())
}

fn publisher<'a>(ctx: &'a Context<'_>) -> Option<&'a Arc<EventPublisher>> {
    ctx.data::<Arc<EventPublisher>>().ok()
}

/// `setAccountSavings`: publish the record (or a tombstone), then project it.
/// Rejects an account that is not the caller's.
pub(crate) async fn set_account_savings(
    ctx: &Context<'_>,
    account_id: Uuid,
    is_savings: bool,
) -> async_graphql::Result<Account> {
    let user = current_user(ctx)?;
    let db: &Arc<DatabaseConnection> = ctx.data()?;
    if !user.account_ids.contains(&account_id) {
        return Err(async_graphql::Error::new(format!("account '{account_id}' is not accessible")));
    }
    let publisher = publisher(ctx).ok_or_else(kafka_unavailable_error)?;
    let key = account_savings_topic_key(user.user_id, account_id);

    if is_savings {
        let record = AccountSavingsRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            user_id: user.user_id,
            account_id,
            revision: Utc::now(),
        };
        let value = serde_json::to_vec(&record).map_err(|e| {
            async_graphql::Error::new(format!("failed to serialize account savings flag: {e}"))
        })?;
        publish_event(publisher, TOPIC_ACCOUNT_SAVINGS, &key, &value).await?;
        proj::project_account_savings(db.as_ref(), user.user_id, account_id, Some(record)).await?;
    } else {
        publish_tombstone(publisher, TOPIC_ACCOUNT_SAVINGS, &key).await?;
        proj::project_account_savings(db.as_ref(), user.user_id, account_id, None).await?;
    }

    accounts::fetch_accounts(db.as_ref(), user.user_id, &[account_id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| async_graphql::Error::new("account vanished"))
}
