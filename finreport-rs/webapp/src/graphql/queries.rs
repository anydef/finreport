//! Query root (§5). Resolver bodies wire the auth context (`current_user`)
//! and scoping (`scoped_account_ids`) to the pure/DB-touching helpers in
//! `accounts.rs`, `transactions.rs` and `cashflow/`.

use async_graphql::{Context, Object, Result as GqlResult};
use entity::entities::app_user;
use sea_orm::{DatabaseConnection, EntityTrait};
use std::sync::Arc;

use crate::graphql::cashflow;
use crate::graphql::current_user::{current_user, scoped_account_ids, AuthenticatedUser};
use crate::graphql::labels::LabelSplitCache;
use crate::graphql::types::{
    Account, CashflowGraph, CashflowGraphInput, CashflowSummary, Category,
    CategoryBreakdown, CategoryKind, Granularity, Me, PageInput, ReviewQueue, Rule, RuleState,
    TransactionFilter, TransactionPage,
};
use crate::graphql::{accounts, breakdown, categories, review_queue, rules, transactions};

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
        let page = transactions::fetch_transactions(db, &scoped_ids, &filter, page.limit, page.offset).await?;

        // Prime the per-request label/split cache for this page's rows
        // (§9 N+1 avoidance) — `Transaction.label`/`.splits` then read from
        // memory instead of issuing one query per row.
        if let Ok(cache) = ctx.data::<LabelSplitCache>() {
            let ids: Vec<uuid::Uuid> = page.items.iter().map(|t| t.id.0).collect();
            crate::graphql::labels::prefetch(db, cache, &ids).await?;
        }
        Ok(page)
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

    /// `categories` (§5): the shared tree is not tenant-scoped (§3's
    /// `owner_user_id` is reserved, always `NULL` this iteration) — any
    /// authenticated user sees the same catalog.
    async fn categories(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = false)] include_archived: bool,
    ) -> GqlResult<Vec<Category>> {
        current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        categories::fetch_categories(db, include_archived).await
    }

    async fn category_breakdown(
        &self,
        ctx: &Context<'_>,
        filter: TransactionFilter,
        #[graphql(default = 1)] level: i32,
        kind: Option<CategoryKind>,
    ) -> GqlResult<CategoryBreakdown> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let requested_ids: Option<Vec<uuid::Uuid>> = filter
            .account_ids
            .as_ref()
            .map(|ids| ids.iter().map(|id| id.0).collect());
        let scoped_ids = scoped_account_ids(user, requested_ids.as_deref())?;
        breakdown::fetch_breakdown(db, &scoped_ids, &filter, level, kind).await
    }

    /// `rules` (§5): like `categories`, not tenant-scoped — rules apply to
    /// every account the labeler sees.
    async fn rules(&self, ctx: &Context<'_>, state: Option<RuleState>) -> GqlResult<Vec<Rule>> {
        current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        rules::fetch_rules(db, state).await
    }

    async fn recently_auto_approved_rules(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 20)] limit: i32,
    ) -> GqlResult<Vec<Rule>> {
        current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        rules::fetch_recently_auto_approved(db, limit).await
    }

    async fn review_queue(&self, ctx: &Context<'_>, page: Option<PageInput>) -> GqlResult<ReviewQueue> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let page = page.unwrap_or_default();
        let scoped_ids = scoped_account_ids(user, None)?;
        let cache = ctx.data::<LabelSplitCache>()?;
        review_queue::fetch_review_queue(db, cache, &scoped_ids, page.limit, page.offset).await
    }
}
