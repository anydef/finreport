//! Authentication core (§4): Argon2id password hashing, session tokens, and
//! user/account administration — plain library functions with no actix / no
//! async-graphql dependency, so WP4 can wire them into the GraphQL layer and
//! `user-admin` (`webapp/src/bin/user_admin.rs`) can drive them from the CLI
//! without either depending on the other.
//!
//! Module map:
//! - [`error`] — [`AuthError`], the single error type every function here
//!   returns.
//! - [`password`] — Argon2id hash/verify.
//! - [`token`] — session token generation + SHA-256 hashing.
//! - [`session`] — session create/verify/revoke/prune.
//! - [`user`] — [`AuthenticatedUser`] loading + `user-admin`'s
//!   create/link/unlink operations.

pub mod error;
pub mod password;
pub mod session;
#[cfg(all(test, feature = "integration"))]
mod session_integration_tests;
pub mod token;
pub mod user;

pub use error::AuthError;
pub use password::{hash_password, verify_password};
pub use session::{create_session, prune_expired_sessions, revoke_session, verify_session};
pub use token::{generate_token, hash_token};
pub use user::{
    AuthenticatedUser, authenticate, create_user, link_account, link_all_accounts, list_accounts,
    list_users, load_authenticated_user, set_password, unlink_account,
};
