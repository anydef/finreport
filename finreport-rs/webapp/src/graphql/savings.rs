//! Savings accounts: which accounts the caller treats as savings
//! (`Account.isSavings`, `setAccountSavings`).
//!
//! The flag is **per user** - a reporting choice, not a fact from the bank -
//! so it lives on its own compacted topic keyed by user + account
//! (`kafka::savings`), projected into `account_savings`. `savingsSummary`
//! reads it back; the arithmetic is in `crate::savings`.

use async_graphql::{Context, ErrorExtensions, SimpleObject};
use chrono::Utc;
use entity::entities::account_savings;
use entity::entities::transaction;
use rust_decimal::Decimal;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect};
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use uuid::Uuid;

use crate::graphql::accounts;
use crate::graphql::current_user::{current_user, AuthenticatedUser};
use crate::graphql::scalars::Decimal as GqlDecimal;
use crate::graphql::transactions::build_condition_with_category_filters;
use crate::graphql::events::{kafka_unavailable_error, publish_event, publish_tombstone};
use crate::graphql::types::{Account, TransactionFilter};
use crate::kafka::producer::EventPublisher;
use crate::kafka::savings::{
    account_savings_topic_key, AccountSavingsRecord, CURRENT_SCHEMA_VERSION, TOPIC_ACCOUNT_SAVINGS,
};
use crate::projection::account_savings as proj;
use crate::savings::{net_movements, Movement};

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

// ---------------------------------------------------------------------------
// savingsSummary (the read side)
// ---------------------------------------------------------------------------

/// One savings account's flows over the period.
#[derive(SimpleObject)]
pub struct SavingsAccountFlow {
    pub account: Account,
    /// Money that arrived in the account.
    pub paid_in: GqlDecimal,
    /// Money that left it, as a positive magnitude.
    pub withdrawn: GqlDecimal,
    /// `paidIn - withdrawn`; negative when the period drew the account down.
    pub net: GqlDecimal,
}

/// Net change across the accounts the caller marked as savings, over the same
/// period and panel filter as the rest of the dashboard.
///
/// Movements between two savings accounts are excluded (they change no
/// total); `internalTransferCount` says how many.
#[derive(SimpleObject)]
pub struct SavingsSummary {
    pub currency: String,
    /// `paidIn - withdrawn` across every savings account.
    pub net_put_aside: GqlDecimal,
    pub paid_in: GqlDecimal,
    pub withdrawn: GqlDecimal,
    pub accounts: Vec<SavingsAccountFlow>,
    /// Savings-to-savings movements excluded as self-cancelling, counted as
    /// movements (two rows are one movement).
    pub internal_transfer_count: i32,
    /// Of `withdrawn`, the part that went to an outside payee rather than to
    /// another of the caller's accounts - so it is also in the spending card.
    pub spent_directly: GqlDecimal,
}

/// id, account, amount, is_transfer, counterpart id, counterpart's account.
type MovementRow = (Uuid, Uuid, Decimal, bool, Option<Uuid>, Option<Uuid>);

/// The savings accounts' rows under the same predicate as the transaction
/// list (`build_condition_with_category_filters`, so period, search, category
/// and the rest agree with it by construction), each with its detected
/// transfer counterpart and that counterpart's account, in **one query**.
///
/// Unlike the cashflow charts this does not drop transfers by default: a
/// transfer into savings is the point. `filter.transfer` still narrows it, as
/// it does on the list.
async fn fetch_movements(
    db: &DatabaseConnection,
    savings_ids: &[Uuid],
    filter: &TransactionFilter,
) -> async_graphql::Result<Vec<Movement>> {
    let condition = build_condition_with_category_filters(db, savings_ids, filter).await?;
    let rows: Vec<MovementRow> =
        transaction::Entity::find()
            .filter(condition)
            .select_only()
            .columns([transaction::Column::Id, transaction::Column::AccountId, transaction::Column::Amount])
            .expr(Expr::cust(
                "COALESCE((SELECT ti.is_transfer FROM transaction_insight ti WHERE ti.transaction_id = transaction.id), false)",
            ))
            .expr(Expr::cust(
                "(SELECT ti.transfer_counterpart_id FROM transaction_insight ti WHERE ti.transaction_id = transaction.id)",
            ))
            .expr(Expr::cust(
                "(SELECT c.account_id FROM transaction_insight ti \
                 JOIN transaction c ON c.id = ti.transfer_counterpart_id \
                 WHERE ti.transaction_id = transaction.id)",
            ))
            .into_tuple()
            .all(db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(transaction_id, account_id, amount, is_transfer, counterpart_id, counterpart_account_id)| Movement {
            transaction_id,
            account_id,
            amount,
            is_transfer,
            counterpart_id,
            counterpart_account_id,
        })
        .collect())
}

/// `savingsSummary`: `None` when the caller has marked no savings account, or
/// the panel's account selection contains none of them.
///
/// **Currency.** An account has one currency and a sum across currencies is
/// meaningless, so the summary is in the currency of the savings accounts in
/// scope and refuses (a `VALIDATION` error naming the currencies) if they
/// differ - narrowing `accountIds` is the way out. It never adds across them.
pub async fn fetch_savings_summary(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
) -> async_graphql::Result<Option<SavingsSummary>> {
    let all_flagged = flagged_accounts(db, user.user_id, &user.account_ids).await?;
    let in_scope: Vec<Uuid> = scoped_ids.iter().copied().filter(|id| all_flagged.contains(id)).collect();
    if in_scope.is_empty() {
        return Ok(None);
    }

    let mut accounts = accounts::fetch_accounts(db, user.user_id, &in_scope).await?;
    accounts.sort_by(|a, b| (&a.label, a.id.0).cmp(&(&b.label, b.id.0)));
    let currencies: BTreeSet<&str> = accounts.iter().map(|a| a.currency.as_str()).collect();
    if currencies.len() > 1 {
        return Err(async_graphql::Error::new(format!(
            "savings accounts span several currencies ({}); select accounts of one currency",
            currencies.into_iter().collect::<Vec<_>>().join(", ")
        ))
        .extend_with(|_, e| e.set("code", "VALIDATION")));
    }
    let currency = accounts[0].currency.clone();

    let movements = fetch_movements(db, &in_scope, filter).await?;
    let order: Vec<Uuid> = accounts.iter().map(|a| a.id.0).collect();
    let netting = net_movements(&order, &all_flagged, &movements);

    let flows = accounts
        .into_iter()
        .zip(&netting.accounts)
        .map(|(account, flow)| SavingsAccountFlow {
            account,
            paid_in: GqlDecimal(flow.paid_in),
            withdrawn: GqlDecimal(flow.withdrawn),
            net: GqlDecimal(flow.net()),
        })
        .collect();
    Ok(Some(SavingsSummary {
        currency,
        net_put_aside: GqlDecimal(netting.net()),
        paid_in: GqlDecimal(netting.paid_in),
        withdrawn: GqlDecimal(netting.withdrawn),
        accounts: flows,
        internal_transfer_count: netting.internal_transfer_count as i32,
        spent_directly: GqlDecimal(netting.spent_directly),
    }))
}
