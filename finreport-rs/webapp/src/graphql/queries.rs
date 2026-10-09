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
    CategoryBreakdown, CategoryKind, Granularity, Me, PageInput, RecurringOverview, ReviewQueue,
    Rule, RuleState, TagCount, TransactionFilter, TransactionPage, TransactionSort,
};
use crate::graphql::comparison::CategoryComparison;
use crate::graphql::scalars::{Date, Uuid};
use crate::graphql::{
    accounts, attention, breakdown, categories, comparison, goals, held_groups, review_queue, rules, transactions,
};

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
        let row = app_user::Entity::find_by_id(user.user_id).one(db).await?;
        let display_name = row.as_ref().and_then(|row| row.display_name.clone());
        let is_admin = row.map(|row| row.is_admin).unwrap_or(false);
        Ok(Some(Me {
            id: user.user_id.into(),
            username: user.username.clone(),
            display_name,
            is_admin,
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
        sort: Option<TransactionSort>,
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
        let page = transactions::fetch_transactions(db, &scoped_ids, &filter, page.limit, page.offset, sort).await?;

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

    /// Spending per category across consecutive calendar periods of
    /// `filter`'s `startDate`..`endDate`, plus each period's total — one
    /// query for a headline trend and for "which categories drove it".
    /// `kind` defaults to `EXPENSE`. Same scoping and rules as
    /// `categoryBreakdown`, which it is built on.
    async fn category_comparison(
        &self,
        ctx: &Context<'_>,
        filter: TransactionFilter,
        #[graphql(default_with = "Granularity::Month")] granularity: Granularity,
        #[graphql(default = 1)] level: i32,
        #[graphql(default_with = "CategoryKind::Expense")] kind: CategoryKind,
    ) -> GqlResult<CategoryComparison> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let requested_ids: Option<Vec<uuid::Uuid>> = filter
            .account_ids
            .as_ref()
            .map(|ids| ids.iter().map(|id| id.0).collect());
        let scoped_ids = scoped_account_ids(user, requested_ids.as_deref())?;
        comparison::fetch_comparison(db, &scoped_ids, filter, granularity, level, kind).await
    }

    /// `rules` (§5): like `categories`, not tenant-scoped — rules apply to
    /// every account the labeler sees.
    async fn rules(&self, ctx: &Context<'_>, state: Option<RuleState>) -> GqlResult<Vec<Rule>> {
        current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        rules::fetch_rules(db, state).await
    }

    /// `learningExemptions`: merchants the user has told the learner to
    /// leave alone. Like `rules`, the list itself is not tenant-scoped; the
    /// per-merchant name and count are computed over the caller's accounts.
    async fn learning_exemptions(
        &self,
        ctx: &Context<'_>,
    ) -> GqlResult<Vec<crate::graphql::learning_exemptions::LearningExemption>> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let scoped_ids = scoped_account_ids(user, None)?;
        crate::graphql::learning_exemptions::fetch_learning_exemptions(db, &scoped_ids).await
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

    /// What is waiting for the user: uncategorised and held-for-review
    /// transactions with their worth. All-time, across the caller's accounts.
    async fn attention_summary(&self, ctx: &Context<'_>) -> GqlResult<attention::AttentionSummary> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let scoped_ids = scoped_account_ids(user, None)?;
        attention::fetch_attention_summary(db, &scoped_ids).await
    }

    /// `heldMerchantGroups`: the held (`NEEDS_REVIEW`) transactions of the
    /// caller's own accounts grouped by merchant (`counterpartyKey`), largest
    /// group first. Held transactions with no key form one extra bucket with a
    /// `null` key, so the groups always sum to `heldCount`. `page` pages the
    /// groups, not the transactions.
    async fn held_merchant_groups(
        &self,
        ctx: &Context<'_>,
        page: Option<PageInput>,
    ) -> GqlResult<held_groups::HeldMerchantGroups> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let page = page.unwrap_or_default();
        let scoped_ids = scoped_account_ids(user, None)?;
        held_groups::fetch_held_merchant_groups(db, &scoped_ids, page.limit, page.offset).await
    }

    /// `tags` (§4): all tags, descending count.
    async fn tags(&self, ctx: &Context<'_>) -> GqlResult<Vec<TagCount>> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let scoped_ids = scoped_account_ids(user, None)?;
        crate::graphql::insights::fetch_tag_counts(db, &scoped_ids).await
    }

    /// `goals` (iteration 4 §4): the caller's own goals.
    async fn goals(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = false)] include_archived: bool,
    ) -> GqlResult<Vec<goals::Goal>> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        goals::goals(db, user, include_archived).await
    }

    async fn goal(&self, ctx: &Context<'_>, id: Uuid) -> GqlResult<Option<goals::Goal>> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        goals::goal(db, user, id).await
    }

    /// Window defaults to the goal's own period for FIXED, last 12 periods for RECURRING.
    async fn goal_progress(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        start_date: Option<Date>,
        end_date: Option<Date>,
    ) -> GqlResult<goals::GoalProgress> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        goals::goal_progress(db, user, id, start_date, end_date).await
    }

    /// The contribution rows behind a bucket, for drill-down.
    async fn goal_transactions(
        &self,
        ctx: &Context<'_>,
        id: Uuid,
        start_date: Date,
        end_date: Date,
        page: Option<PageInput>,
        sort: Option<TransactionSort>,
    ) -> GqlResult<TransactionPage> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        goals::goal_transactions(db, user, id, start_date, end_date, page, sort).await
    }

    /// `recurringSeries` (§4).
    async fn recurring_series(
        &self,
        ctx: &Context<'_>,
        filter: Option<TransactionFilter>,
    ) -> GqlResult<RecurringOverview> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let filter = filter.unwrap_or_default();
        let requested_ids: Option<Vec<uuid::Uuid>> = filter
            .account_ids
            .as_ref()
            .map(|ids| ids.iter().map(|id| id.0).collect());
        let scoped_ids = scoped_account_ids(user, requested_ids.as_deref())?;
        crate::graphql::insights::fetch_recurring_overview(db, &scoped_ids, &filter).await
    }
}
