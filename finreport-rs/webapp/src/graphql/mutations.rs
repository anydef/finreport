//! Mutation root (§5). `login`/`logout` call `auth_shim` (WP2's temporary
//! stand-in, see that module's doc comment) and set/clear the session
//! cookie via `ctx.append_http_header`, which the actix integration
//! propagates onto the HTTP response (§4).

use async_graphql::{Context, ErrorExtensions, Object, Result as GqlResult};
use sea_orm::DatabaseConnection;
use secrecy::SecretString;
use std::sync::Arc;
use utils::settings::Settings;

use crate::graphql::auth_shim;
use crate::graphql::types::{LoginInput, Me};
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

        match auth_shim::authenticate(
            db,
            &input.username,
            &password,
            settings.session_ttl_days,
            user_agent,
        )
        .await
        {
            Ok((user, token)) => {
                ctx.append_http_header(
                    "set-cookie",
                    auth_shim::cookie_header(&token, settings.session_ttl_days, settings.cookie_secure),
                );
                Ok(Me {
                    id: user.user_id.into(),
                    username: user.username,
                    display_name: user.display_name,
                })
            }
            Err(_) => Err(invalid_credentials()),
        }
    }

    async fn logout(&self, ctx: &Context<'_>) -> GqlResult<bool> {
        let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
        let settings = ctx.data::<Arc<Settings>>()?;
        if let Ok(token) = ctx.data::<RawSessionToken>() {
            if let Some(raw) = &token.raw {
                auth_shim::revoke_session(db, raw).await?;
            }
        }
        ctx.append_http_header(
            "set-cookie",
            auth_shim::clear_cookie_header(settings.cookie_secure),
        );
        Ok(true)
    }
}

fn invalid_credentials() -> async_graphql::Error {
    async_graphql::Error::new("invalid username or password")
        .extend_with(|_, e| e.set("code", "INVALID_CREDENTIALS"))
}
