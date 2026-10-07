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
    not_implemented, Account, CashflowGraph, CashflowGraphInput, CashflowSummary, Category,
    CategoryBreakdown, CategoryKind, Granularity, Me, PageInput, ReviewQueue, Rule, RuleState,
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

    /// TODO(WP4): list categories from the `category` projection (§3),
    /// filtered to non-archived unless `includeArchived`.
    async fn categories(
        &self,
        _ctx: &Context<'_>,
        #[graphql(default = false)] include_archived: bool,
    ) -> GqlResult<Vec<Category>> {
        let _ = include_archived;
        Err(not_implemented("Query.categories", 4))
    }

    /// TODO(WP4): roll up `transaction_label`/`transaction_split` by
    /// category to `level`, honouring §5's split/transfer/share semantics.
    async fn category_breakdown(
        &self,
        _ctx: &Context<'_>,
        filter: TransactionFilter,
        #[graphql(default = 1)] level: i32,
        kind: Option<CategoryKind>,
    ) -> GqlResult<CategoryBreakdown> {
        let _ = (filter, level, kind);
        Err(not_implemented("Query.categoryBreakdown", 4))
    }

    /// TODO(WP4): list rules from the `rule` projection (§2.7), optionally
    /// filtered by `state`.
    async fn rules(&self, _ctx: &Context<'_>, state: Option<RuleState>) -> GqlResult<Vec<Rule>> {
        let _ = state;
        Err(not_implemented("Query.rules", 4))
    }

    /// TODO(WP4): rules auto-approved recently (§2.8), newest first.
    async fn recently_auto_approved_rules(
        &self,
        _ctx: &Context<'_>,
        #[graphql(default = 20)] limit: i32,
    ) -> GqlResult<Vec<Rule>> {
        let _ = limit;
        Err(not_implemented("Query.recentlyAutoApprovedRules", 4))
    }

    /// TODO(WP4): `status = NEEDS_REVIEW` transactions + `state = IN_REVIEW`
    /// rules (§5).
    async fn review_queue(
        &self,
        _ctx: &Context<'_>,
        page: Option<PageInput>,
    ) -> GqlResult<ReviewQueue> {
        let _ = page;
        Err(not_implemented("Query.reviewQueue", 4))
    }
}
