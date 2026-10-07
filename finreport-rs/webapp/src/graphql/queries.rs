//! Query root (§5). Resolver bodies wire the auth context (`current_user`)
//! and scoping (`scoped_account_ids`) to the pure/DB-touching helpers in
//! `accounts.rs`, `transactions.rs` and `cashflow/`.

use async_graphql::{Context, Object, Result as GqlResult};
use entity::entities::app_user;
use sea_orm::{DatabaseConnection, EntityTrait};
use std::sync::Arc;

use crate::graphql::cashflow;
use crate::graphql::current_user::{current_user, scoped_account_ids, AuthenticatedUser};
use crate::graphql::types::{
    Account, CashflowGraph, CashflowGraphInput, CashflowSummary, Granularity, Me, PageInput,
    TransactionFilter, TransactionPage,
};
use crate::graphql::{accounts, transactions};

pub struct QueryRoot;

#[Object(name = "Query")]
impl QueryRoot {
    /// Unauthenticated field (§4/§5): returns the caller's own identity, or
    /// `null` when there is no valid session. `AuthenticatedUser` (§4) does
    /// not carry `display_name` — it's loaded here with one extra query,
    /// the only place in the schema that needs it.
    async fn me(&self, ctx: &Context<'_>) -> GqlResult<Option<Me>> {
        let user = match ctx.data::<Option<AuthenticatedUser>>()? {
            Some(u) => u,
            None => return Ok(None),
        };
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let display_name = app_user::Entity::find_by_id(user.user_id)
            .one(db)
            .await?
            .and_then(|row| row.display_name);
        Ok(Some(Me {
            id: user.user_id.into(),
            username: user.username.clone(),
            display_name,
        }))
    }

    async fn accounts(&self, ctx: &Context<'_>) -> GqlResult<Vec<Account>> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let scoped_ids = scoped_account_ids(user, None)?;
        accounts::fetch_accounts(db, &scoped_ids).await
    }

    async fn transactions(
        &self,
        ctx: &Context<'_>,
        filter: Option<TransactionFilter>,
        page: Option<PageInput>,
    ) -> GqlResult<TransactionPage> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let filter = filter.unwrap_or_default();
        let page = page.unwrap_or_default();
        let requested_ids: Option<Vec<uuid::Uuid>> = filter
            .account_ids
            .as_ref()
            .map(|ids| ids.iter().map(|id| id.0).collect());
        let scoped_ids = scoped_account_ids(user, requested_ids.as_deref())?;
        transactions::fetch_transactions(db, &scoped_ids, &filter, page.limit, page.offset).await
    }

    async fn cashflow_summary(
        &self,
        ctx: &Context<'_>,
        filter: TransactionFilter,
        granularity: Granularity,
    ) -> GqlResult<CashflowSummary> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let requested_ids: Option<Vec<uuid::Uuid>> = filter
            .account_ids
            .as_ref()
            .map(|ids| ids.iter().map(|id| id.0).collect());
        let scoped_ids = scoped_account_ids(user, requested_ids.as_deref())?;
        cashflow::fetch_summary(db, &scoped_ids, &filter, granularity).await
    }

    async fn cashflow_graph(
        &self,
        ctx: &Context<'_>,
        filter: TransactionFilter,
        grouping: Option<CashflowGraphInput>,
    ) -> GqlResult<CashflowGraph> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let requested_ids: Option<Vec<uuid::Uuid>> = filter
            .account_ids
            .as_ref()
            .map(|ids| ids.iter().map(|id| id.0).collect());
        let scoped_ids = scoped_account_ids(user, requested_ids.as_deref())?;
        cashflow::fetch_graph(db, &scoped_ids, &filter, grouping).await
    }
}
