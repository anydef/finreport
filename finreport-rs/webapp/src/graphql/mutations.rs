//! Mutation root (§5). `login`/`logout` are stubs for WP2 (auth core) to
//! replace with real Argon2id verification and session-cookie issuance.

use async_graphql::{Context, ErrorExtensions, Object, Result as GqlResult};

use crate::graphql::types::{LoginInput, Me};

pub struct MutationRoot;

#[Object(name = "Mutation")]
impl MutationRoot {
    async fn login(&self, _ctx: &Context<'_>, _input: LoginInput) -> GqlResult<Me> {
        Err(async_graphql::Error::new("login: not implemented (WP2)")
            .extend_with(|_, e| e.set("code", "NOT_IMPLEMENTED")))
    }

    async fn logout(&self, _ctx: &Context<'_>) -> GqlResult<bool> {
        Err(async_graphql::Error::new("logout: not implemented (WP2)")
            .extend_with(|_, e| e.set("code", "NOT_IMPLEMENTED")))
    }
}
