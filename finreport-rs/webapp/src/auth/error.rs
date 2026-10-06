//! Errors shared by every `auth` submodule.

use std::fmt;

/// Everything that can go wrong in the auth core.
///
/// Deliberately coarse-grained: callers (the GraphQL layer, the CLI) map this
/// onto their own user-facing errors rather than matching on hashing/DB
/// internals. Never carries a password or raw token — only usernames/ids,
/// which are not secret.
#[derive(Debug)]
pub enum AuthError {
    /// Username/password did not match. Deliberately the same variant for
    /// "no such user" and "wrong password" so callers cannot distinguish the
    /// two (no username enumeration via error type).
    InvalidCredentials,
    /// The user exists and the password matched, but `app_user.disabled` is
    /// true.
    UserDisabled,
    /// `username` is already taken (`create_user`).
    UsernameTaken(String),
    /// No `app_user` row for this username (admin lookups).
    UnknownUser(String),
    /// No `account` row for this `source:external_id` (admin `link`/`unlink`).
    UnknownAccount(String, String),
    /// Session token did not resolve to a live, non-expired session.
    SessionNotFound,
    /// Argon2 hashing/verification failed for a reason other than a password
    /// mismatch (e.g. a corrupt PHC string read back from the DB).
    Hash(argon2::password_hash::Error),
    /// Any database error, passed through.
    Db(sea_orm::DbErr),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthError::InvalidCredentials => write!(f, "invalid username or password"),
            AuthError::UserDisabled => write!(f, "user is disabled"),
            AuthError::UsernameTaken(u) => write!(f, "username {u:?} is already taken"),
            AuthError::UnknownUser(u) => write!(f, "no such user {u:?}"),
            AuthError::UnknownAccount(source, external_id) => {
                write!(f, "no such account {source}:{external_id}")
            }
            AuthError::SessionNotFound => write!(f, "session not found or expired"),
            AuthError::Hash(e) => write!(f, "password hashing error: {e}"),
            AuthError::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<argon2::password_hash::Error> for AuthError {
    fn from(e: argon2::password_hash::Error) -> Self {
        AuthError::Hash(e)
    }
}

impl From<sea_orm::DbErr> for AuthError {
    fn from(e: sea_orm::DbErr) -> Self {
        AuthError::Db(e)
    }
}
