mod accounts;
pub mod auth_shim;
mod cashflow;
pub mod current_user;
pub mod http;
mod loaders;
mod mutations;
mod queries;
pub mod scalars;
pub mod transactions;
pub mod types;

use crate::graphql::current_user::AuthenticatedUser;
use crate::graphql::mutations::MutationRoot;
use crate::graphql::queries::QueryRoot;
use async_graphql::{EmptySubscription, Schema};
use sea_orm::DatabaseConnection;
use std::sync::Arc;
use utils::settings::Settings;

pub type AppSchema = Schema<QueryRoot, MutationRoot, EmptySubscription>;

/// The raw `fr_session` cookie value for the current HTTP request (if any),
/// plus the request's `User-Agent` — injected per-request (not at schema-build
/// time) by the actix handler in `main.rs`, alongside the already-resolved
/// `Option<AuthenticatedUser>`. `logout` needs the raw token to revoke the
/// right session row; `login` records the user agent on the new one (§4).
#[derive(Clone, Debug, Default)]
pub struct RawSessionToken {
    pub raw: Option<String>,
    pub user_agent: Option<String>,
}

pub fn create_schema(conn: Arc<DatabaseConnection>, settings: Arc<Settings>) -> AppSchema {
    Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        // `DateTime` (§5) isn't reachable from any field yet — nothing in
        // this iteration's resolvers returns a raw timestamp — so it needs
        // an explicit registration or the exporter would silently drop it
        // from the SDL.
        .register_output_type::<scalars::DateTime>()
        .data(conn)
        .data(settings)
        .finish()
}

/// Per-request context data: the already-resolved auth user (or `None`) and
/// the raw session token, both computed once per request from the `Cookie`
/// header by the actix handler before `schema.execute()` (§4).
pub fn request_with_auth(
    request: async_graphql::Request,
    user: Option<AuthenticatedUser>,
    raw_token: RawSessionToken,
) -> async_graphql::Request {
    request.data(user).data(raw_token)
}
