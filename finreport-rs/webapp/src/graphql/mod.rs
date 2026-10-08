mod attention;
mod accounts;
mod breakdown;
mod bulk;
pub(crate) mod cashflow;
pub mod categories;
pub mod cookies;
pub mod current_user;
mod events;
mod held_groups;
pub mod goals;
pub mod http;
mod insights;
mod labels;
mod loaders;
mod mutations;
mod queries;
mod review_queue;
mod rules;
pub mod scalars;
pub mod transactions;
pub mod types;

use crate::auth::AuthenticatedUser;
use crate::graphql::labels::LabelSplitCache;
use crate::graphql::mutations::MutationRoot;
use crate::graphql::queries::QueryRoot;
use crate::kafka::producer::EventPublisher;
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
    // Best-effort (§2.6/WP4 TODO note): an unreachable broker at startup
    // must not stop `dev-be` from serving read-only queries, only the
    // mutations that publish events — those fail per-request instead with
    // `KAFKA_UNAVAILABLE` (`events::kafka_unavailable_error`).
    let publisher: Option<Arc<EventPublisher>> = settings
        .kafka_brokers
        .as_deref()
        .and_then(|brokers| match EventPublisher::connect(brokers) {
            Ok(publisher) => Some(Arc::new(publisher)),
            Err(e) => {
                tracing::warn!(error = %e, "failed to connect to Kafka; mutations will fail with KAFKA_UNAVAILABLE");
                None
            }
        });

    let mut builder = Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        // `DateTime` (§5) isn't reachable from any field yet — nothing in
        // this iteration's resolvers returns a raw timestamp — so it needs
        // an explicit registration or the exporter would silently drop it
        // from the SDL.
        .register_output_type::<scalars::DateTime>()
        .data(conn)
        .data(settings);
    if let Some(publisher) = publisher {
        builder = builder.data(publisher);
    }
    builder.finish()
}

/// Per-request context data: the already-resolved auth user (or `None`), the
/// raw session token (both computed once per request from the `Cookie`
/// header by the actix handler before `schema.execute()`, §4), and a fresh
/// `LabelSplitCache` for `Transaction.label`/`.splits` prefetching (§9).
pub fn request_with_auth(
    request: async_graphql::Request,
    user: Option<AuthenticatedUser>,
    raw_token: RawSessionToken,
) -> async_graphql::Request {
    request
        .data(user)
        .data(raw_token)
        .data(LabelSplitCache::default())
        .data(rules::RuleReachCache::default())
}
