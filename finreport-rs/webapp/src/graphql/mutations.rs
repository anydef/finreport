//! Mutation root (§5). `login`/`logout` call `crate::auth` (WP2's real auth
//! core) and set/clear the session cookie via `ctx.append_http_header`,
//! which the actix integration propagates onto the HTTP response (§4).
//! Every other mutation here follows §2.1's publish-then-upsert: publish to
//! Kafka first (awaiting the ack), then apply the identical upsert to the
//! caller's own request so the fresh row reads back correctly without
//! waiting on WP3's projector.

use async_graphql::{Context, ErrorExtensions, Object, Result as GqlResult};
use sea_orm::DatabaseConnection;
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use utils::settings::Settings;

use crate::auth;
use crate::graphql::cookies::{clear_cookie_header, cookie_header};
use crate::graphql::current_user::{current_user, scoped_account_ids};
use crate::graphql::types::{
    Category, CategoryInput, LoginInput, Me, Rule, RuleInput, RuleState, SplitPartInput,
    Transaction, TransactionFilter,
};
use crate::graphql::bulk::{self, BulkEditResult};
use crate::graphql::{categories, labels, rules};
use crate::graphql::goals;
use crate::graphql::RawSessionToken;
use crate::kafka::producer::EventPublisher;

pub struct MutationRoot;

#[Object(name = "Mutation")]
impl MutationRoot {
    /// `login`/`logout` are the only unauthenticated mutations (§4/§5). A
    /// bad username and a bad password return the same
    /// `extensions.code = "INVALID_CREDENTIALS"` with the same message — no
    /// user enumeration.
    async fn login(&self, ctx: &Context<'_>, input: LoginInput) -> GqlResult<Me> {
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let settings = ctx.data::<Arc<Settings>>()?;
        let user_agent = ctx
            .data::<RawSessionToken>()
            .ok()
            .and_then(|t| t.user_agent.clone());
        let password = SecretString::from(input.password);

        let user = match auth::authenticate(db, &input.username, &password).await {
            Ok(user) => user,
            Err(_) => return Err(invalid_credentials()),
        };

        let token = auth::create_session(db, user.id, settings.session_ttl_days, user_agent)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        ctx.append_http_header(
            "set-cookie",
            cookie_header(
                token.expose_secret(),
                settings.session_ttl_days,
                settings.cookie_secure,
            ),
        );
        Ok(Me {
            id: user.id.into(),
            username: user.username,
            display_name: user.display_name,
            is_admin: user.is_admin,
        })
    }

    async fn logout(&self, ctx: &Context<'_>) -> GqlResult<bool> {
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let settings = ctx.data::<Arc<Settings>>()?;
        if let Ok(token) = ctx.data::<RawSessionToken>() {
            if let Some(raw) = &token.raw {
                auth::revoke_session(db, raw)
                    .await
                    .map_err(|e| async_graphql::Error::new(e.to_string()))?;
            }
        }
        ctx.append_http_header("set-cookie", clear_cookie_header(settings.cookie_secure));
        Ok(true)
    }

    async fn set_transaction_category(
        &self,
        ctx: &Context<'_>,
        transaction_id: crate::graphql::scalars::Uuid,
        category_slug: String,
    ) -> GqlResult<Transaction> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        let scoped_ids = scoped_account_ids(user, None)?;
        labels::set_transaction_category(db, publisher, &scoped_ids, transaction_id.0, category_slug).await
    }

    async fn clear_transaction_category(
        &self,
        ctx: &Context<'_>,
        transaction_id: crate::graphql::scalars::Uuid,
    ) -> GqlResult<Transaction> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        let scoped_ids = scoped_account_ids(user, None)?;
        labels::clear_transaction_category(db, publisher, &scoped_ids, transaction_id.0).await
    }

    async fn split_transaction(
        &self,
        ctx: &Context<'_>,
        transaction_id: crate::graphql::scalars::Uuid,
        parts: Vec<SplitPartInput>,
    ) -> GqlResult<Transaction> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        let scoped_ids = scoped_account_ids(user, None)?;
        labels::split_transaction(db, publisher, &scoped_ids, transaction_id.0, parts).await
    }

    async fn unsplit_transaction(
        &self,
        ctx: &Context<'_>,
        transaction_id: crate::graphql::scalars::Uuid,
    ) -> GqlResult<Transaction> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        let scoped_ids = scoped_account_ids(user, None)?;
        labels::unsplit_transaction(db, publisher, &scoped_ids, transaction_id.0).await
    }

    async fn create_category(&self, ctx: &Context<'_>, input: CategoryInput) -> GqlResult<Category> {
        current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        categories::create_category(db, publisher, input).await
    }

    /// Idempotent `createCategory` for the review flow ("assign and create
    /// new category"): an equivalent existing category (same kind and parent)
    /// is returned as is; a clashing definition fails with `CONFLICT`.
    async fn ensure_category(&self, ctx: &Context<'_>, input: CategoryInput) -> GqlResult<Category> {
        current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        categories::ensure_category(db, publisher, input).await
    }

    async fn rename_category(
        &self,
        ctx: &Context<'_>,
        id: crate::graphql::scalars::Uuid,
        name: String,
    ) -> GqlResult<Category> {
        current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        categories::rename_category(db, publisher, id.0, name).await
    }

    async fn archive_category(&self, ctx: &Context<'_>, id: crate::graphql::scalars::Uuid) -> GqlResult<Category> {
        current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        categories::archive_category(db, publisher, id.0).await
    }

    async fn create_rule(&self, ctx: &Context<'_>, input: RuleInput) -> GqlResult<Rule> {
        current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        rules::create_rule(db, publisher, input).await
    }

    async fn update_rule(
        &self,
        ctx: &Context<'_>,
        id: crate::graphql::scalars::Uuid,
        input: RuleInput,
    ) -> GqlResult<Rule> {
        current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        rules::update_rule(db, publisher, id.0, input).await
    }

    async fn set_rule_state(
        &self,
        ctx: &Context<'_>,
        id: crate::graphql::scalars::Uuid,
        state: RuleState,
    ) -> GqlResult<Rule> {
        current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        rules::set_rule_state(db, publisher, id.0, state).await
    }

    async fn reapply_rule(&self, ctx: &Context<'_>, id: crate::graphql::scalars::Uuid) -> GqlResult<i32> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        let scoped_ids = scoped_account_ids(user, None)?;
        rules::reapply_rule(db, publisher, &scoped_ids, id.0).await
    }

    /// `exemptFromLearning`: never learn a rule for this merchant again, and
    /// discard its learned rules unless the user has touched them.
    async fn exempt_from_learning(
        &self,
        ctx: &Context<'_>,
        counterparty_key: String,
    ) -> GqlResult<crate::graphql::learning_exemptions::LearningExemption> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        let scoped_ids = scoped_account_ids(user, None)?;
        crate::graphql::learning_exemptions::exempt_from_learning(
            db,
            publisher,
            &scoped_ids,
            user.user_id,
            &counterparty_key,
        )
        .await
    }

    /// `removeLearningExemption`: let the learner work on this merchant
    /// again. True when an exemption existed.
    async fn remove_learning_exemption(
        &self,
        ctx: &Context<'_>,
        counterparty_key: String,
    ) -> GqlResult<bool> {
        current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        crate::graphql::learning_exemptions::remove_learning_exemption(db, publisher, &counterparty_key).await
    }

    /// `setDisplayAlias`: nickname a merchant (`COUNTERPARTY`, by name or
    /// normalised key; covers every spelling) or one of the caller's accounts
    /// (`ACCOUNT`, by id). Display only.
    async fn set_display_alias(
        &self,
        ctx: &Context<'_>,
        kind: crate::graphql::display_aliases::GqlAliasKind,
        key: String,
        alias: String,
    ) -> GqlResult<crate::graphql::display_aliases::DisplayAlias> {
        crate::graphql::display_aliases::set(ctx, kind, &key, &alias).await
    }

    /// `removeDisplayAlias`: the original name shows again. True when an
    /// alias existed.
    async fn remove_display_alias(
        &self,
        ctx: &Context<'_>,
        kind: crate::graphql::display_aliases::GqlAliasKind,
        key: String,
    ) -> GqlResult<bool> {
        crate::graphql::display_aliases::remove(ctx, kind, &key).await
    }

    /// `createGoal` (iteration 4 §4).
    async fn create_goal(
        &self,
        ctx: &Context<'_>,
        input: goals::GoalInput,
    ) -> GqlResult<goals::Goal> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let settings = ctx.data::<Arc<Settings>>()?;
        goals::create_goal(db, publisher(ctx), user, settings.max_tags_per_transaction, input).await
    }

    /// `updateGoal` (iteration 4 §4): read-modify-write on `finreport.goal`.
    async fn update_goal(
        &self,
        ctx: &Context<'_>,
        id: crate::graphql::scalars::Uuid,
        input: goals::GoalInput,
    ) -> GqlResult<goals::Goal> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let settings = ctx.data::<Arc<Settings>>()?;
        goals::update_goal(db, publisher(ctx), user, settings.max_tags_per_transaction, id, input)
            .await
    }

    async fn archive_goal(
        &self,
        ctx: &Context<'_>,
        id: crate::graphql::scalars::Uuid,
    ) -> GqlResult<goals::Goal> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        goals::archive_goal(db, publisher(ctx), user, id).await
    }

    /// Replaces the whole tag set; `[]` clears (§4).
    async fn set_transaction_tags(
        &self,
        ctx: &Context<'_>,
        transaction_id: crate::graphql::scalars::Uuid,
        tags: Vec<String>,
    ) -> GqlResult<Transaction> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let settings = ctx.data::<Arc<Settings>>()?;
        let publisher = publisher(ctx);
        let scoped_ids = scoped_account_ids(user, None)?;
        crate::graphql::insights::set_transaction_tags(
            db,
            publisher,
            &scoped_ids,
            transaction_id.0,
            tags,
            settings.max_tags_per_transaction,
        )
        .await
    }

    /// Overrides the category of every transaction the filter matches (and
    /// clears their splits). Restricted to the caller's own accounts; not
    /// atomic — see `BulkEditResult`.
    async fn set_transactions_category(
        &self,
        ctx: &Context<'_>,
        filter: TransactionFilter,
        category_slug: String,
    ) -> GqlResult<BulkEditResult> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let settings = ctx.data::<Arc<Settings>>()?;
        bulk::set_transactions_category(
            db,
            publisher(ctx),
            user,
            &filter,
            &category_slug,
            settings.bulk_edit_max_transactions,
        )
        .await
    }

    /// Replaces the tag set of every transaction the filter matches; `[]`
    /// clears. Restricted to the caller's own accounts; not atomic.
    async fn set_transactions_tags(
        &self,
        ctx: &Context<'_>,
        filter: TransactionFilter,
        tags: Vec<String>,
    ) -> GqlResult<BulkEditResult> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let settings = ctx.data::<Arc<Settings>>()?;
        bulk::set_transactions_tags(
            db,
            publisher(ctx),
            user,
            &filter,
            &tags,
            settings.max_tags_per_transaction,
            settings.bulk_edit_max_transactions,
        )
        .await
    }

    /// Sets the free-text note on a transaction; `null` or blank clears it.
    /// Commentary only: category, tags, splits and recurring are untouched
    /// and labelling never reads it.
    async fn set_transaction_note(
        &self,
        ctx: &Context<'_>,
        transaction_id: crate::graphql::scalars::Uuid,
        note: Option<String>,
    ) -> GqlResult<Transaction> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        let scoped_ids = scoped_account_ids(user, None)?;
        crate::graphql::insights::set_transaction_note(db, publisher, &scoped_ids, transaction_id.0, note).await
    }

    /// `null` clears the override and lets auto-detection decide again
    /// (§4).
    async fn set_transaction_recurring(
        &self,
        ctx: &Context<'_>,
        transaction_id: crate::graphql::scalars::Uuid,
        recurring: Option<bool>,
    ) -> GqlResult<Transaction> {
        let user = current_user(ctx)?;
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let publisher = publisher(ctx);
        let scoped_ids = scoped_account_ids(user, None)?;
        crate::graphql::insights::set_transaction_recurring(
            db,
            publisher,
            &scoped_ids,
            transaction_id.0,
            recurring,
        )
        .await
    }
}

/// `None` when no broker is configured (`APP_kafka_brokers` unset, e.g.
/// local `dev-be`) — every mutation function treats that as
/// `KAFKA_UNAVAILABLE` rather than panicking.
fn publisher<'a>(ctx: &'a Context<'_>) -> Option<&'a Arc<EventPublisher>> {
    ctx.data::<Arc<EventPublisher>>().ok()
}

fn invalid_credentials() -> async_graphql::Error {
    async_graphql::Error::new("invalid username or password")
        .extend_with(|_, e| e.set("code", "INVALID_CREDENTIALS"))
}
