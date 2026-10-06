//! actix wiring for the GraphQL endpoint (§4/§5): cookie extraction, the
//! CORS allow-list, and the request handler. Pulled out of `main.rs` so the
//! HTTP integration tests in `webapp/tests/` exercise the exact same code
//! path the real server runs, not a re-implementation of it.

use actix_cors::Cors;
use actix_web::http::header;
use actix_web::{web, HttpRequest};
use async_graphql_actix_web::{GraphQLRequest, GraphQLResponse};
use sea_orm::DatabaseConnection;
use std::sync::Arc;
use utils::settings::Settings;

use crate::graphql::{auth_shim, request_with_auth, AppSchema, RawSessionToken};

/// Extracts `fr_session` from the raw `Cookie` header, looks up its session
/// (via `auth_shim`, WP2's temporary stand-in — see that module) and injects
/// `Option<AuthenticatedUser>` plus the raw token into the GraphQL request
/// context (§4). Every resolver but `me`/`login`/`logout` goes through that
/// context — there is no unscoped query path.
pub async fn graphql_handler(
    schema: web::Data<AppSchema>,
    db: web::Data<Arc<DatabaseConnection>>,
    http_req: HttpRequest,
    gql_req: GraphQLRequest,
) -> GraphQLResponse {
    let raw_token = http_req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(auth_shim::extract_token);
    let user_agent = http_req
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let auth_user = match &raw_token {
        Some(token) => auth_shim::session_user(&db, token).await.unwrap_or(None),
        None => None,
    };

    let request = request_with_auth(
        gql_req.into_inner(),
        auth_user,
        RawSessionToken {
            raw: raw_token,
            user_agent,
        },
    );
    schema.execute(request).await.into()
}

/// Explicit allow-list + credentials, never `allow_any_origin()` — that
/// combination is rejected by browsers anyway, and would otherwise accept a
/// session cookie from anywhere (§4).
pub fn cors(settings: &Settings) -> Cors {
    let mut cors = Cors::default()
        .allowed_methods(vec!["GET", "POST"])
        .allowed_headers(vec![header::CONTENT_TYPE, header::ORIGIN])
        .supports_credentials();
    for origin in settings.allowed_origins() {
        cors = cors.allowed_origin(&origin);
    }
    cors
}
