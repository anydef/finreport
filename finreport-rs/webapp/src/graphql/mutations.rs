//! Mutation root (§5). `login`/`logout` call `crate::auth` (WP2's real auth
//! core) and set/clear the session cookie via `ctx.append_http_header`,
//! which the actix integration propagates onto the HTTP response (§4).

use async_graphql::{Context, ErrorExtensions, Object, Result as GqlResult};
use sea_orm::DatabaseConnection;
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use utils::settings::Settings;

use crate::auth;
use crate::graphql::cookies::{clear_cookie_header, cookie_header};
use crate::graphql::types::{
    not_implemented, Category, CategoryInput, LoginInput, Me, Rule, RuleInput, RuleState,
    SplitPartInput, Transaction,
};
use crate::graphql::RawSessionToken;

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

    /// TODO(WP4): publish a `user-label` record setting the category,
    /// publish-then-upsert (§2.1), return the fresh row.
    async fn set_transaction_category(
        &self,
        _ctx: &Context<'_>,
        transaction_id: crate::graphql::scalars::Uuid,
        category_slug: String,
    ) -> GqlResult<Transaction> {
        let _ = (transaction_id, category_slug);
        Err(not_implemented("Mutation.setTransactionCategory", 4))
    }

    /// TODO(WP4): publish a `user-label` tombstone/clear for this
    /// transaction (§2.6).
    async fn clear_transaction_category(
        &self,
        _ctx: &Context<'_>,
        transaction_id: crate::graphql::scalars::Uuid,
    ) -> GqlResult<Transaction> {
        let _ = transaction_id;
        Err(not_implemented("Mutation.clearTransactionCategory", 4))
    }

    /// TODO(WP4): publish a `user-label` split record; validate the sum
    /// against the transaction's amount (`extensions.code =
    /// "SPLIT_SUM_MISMATCH"` on mismatch, §5).
    async fn split_transaction(
        &self,
        _ctx: &Context<'_>,
        transaction_id: crate::graphql::scalars::Uuid,
        parts: Vec<SplitPartInput>,
    ) -> GqlResult<Transaction> {
        let _ = (transaction_id, parts);
        Err(not_implemented("Mutation.splitTransaction", 4))
    }

    /// TODO(WP4): publish a `user-label` record clearing the split (§2.6).
    async fn unsplit_transaction(
        &self,
        _ctx: &Context<'_>,
        transaction_id: crate::graphql::scalars::Uuid,
    ) -> GqlResult<Transaction> {
        let _ = transaction_id;
        Err(not_implemented("Mutation.unsplitTransaction", 4))
    }

    /// TODO(WP4): validate depth/slug-shape/parent-kind/parent-archived
    /// (§5), publish a `category` record, return the fresh row.
    async fn create_category(
        &self,
        _ctx: &Context<'_>,
        input: CategoryInput,
    ) -> GqlResult<Category> {
        let _ = input;
        Err(not_implemented("Mutation.createCategory", 4))
    }

    /// TODO(WP4): publish a renamed `category` record; `slug` is immutable
    /// (§3), only `name` changes.
    async fn rename_category(
        &self,
        _ctx: &Context<'_>,
        id: crate::graphql::scalars::Uuid,
        name: String,
    ) -> GqlResult<Category> {
        let _ = (id, name);
        Err(not_implemented("Mutation.renameCategory", 4))
    }

    /// TODO(WP4): publish a `category` record with `archived = true`;
    /// existing labels keep rendering it (§5).
    async fn archive_category(
        &self,
        _ctx: &Context<'_>,
        id: crate::graphql::scalars::Uuid,
    ) -> GqlResult<Category> {
        let _ = id;
        Err(not_implemented("Mutation.archiveCategory", 4))
    }

    /// TODO(WP4): publish a new user-origin `rule` record (§2.7).
    async fn create_rule(&self, _ctx: &Context<'_>, input: RuleInput) -> GqlResult<Rule> {
        let _ = input;
        Err(not_implemented("Mutation.createRule", 4))
    }

    /// TODO(WP4): publish an updated `rule` record; marks `userTouched`
    /// (§2.8) so the learner never overwrites it.
    async fn update_rule(
        &self,
        _ctx: &Context<'_>,
        id: crate::graphql::scalars::Uuid,
        input: RuleInput,
    ) -> GqlResult<Rule> {
        let _ = (id, input);
        Err(not_implemented("Mutation.updateRule", 4))
    }

    /// TODO(WP4): publish a `rule` record with the new `state`; a revoke
    /// makes the labeler re-resolve every transaction it had labelled
    /// (§2.3/§2.7).
    async fn set_rule_state(
        &self,
        _ctx: &Context<'_>,
        id: crate::graphql::scalars::Uuid,
        state: RuleState,
    ) -> GqlResult<Rule> {
        let _ = (id, state);
        Err(not_implemented("Mutation.setRuleState", 4))
    }

    /// TODO(WP4): publish one `label-request` keyed `reapply:<rule-id>` and
    /// return the number of transactions it will cover (§2.3/§5) —
    /// re-resolution is asynchronous, this mutation never calls the
    /// labeler directly.
    async fn reapply_rule(
        &self,
        _ctx: &Context<'_>,
        id: crate::graphql::scalars::Uuid,
    ) -> GqlResult<i32> {
        let _ = id;
        Err(not_implemented("Mutation.reapplyRule", 4))
    }
}

fn invalid_credentials() -> async_graphql::Error {
    async_graphql::Error::new("invalid username or password")
        .extend_with(|_, e| e.set("code", "INVALID_CREDENTIALS"))
}
