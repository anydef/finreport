//! Session lifecycle: create/verify/revoke/prune (§4).
//!
//! On login: 32 random bytes, base64url — that string is the cookie value
//! and is never stored; Postgres keeps `sha256(token)` in
//! `user_session.token_hash`. TTL 30 days by default, with a sliding
//! `last_seen_at` refresh at most once per hour (no write per request).
//! Logout deletes the row; expired rows are pruned opportunistically on
//! login.

use chrono::{DateTime, Duration, Utc};
use entity::entities::{app_user, user_session};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, ModelTrait, QueryFilter, Set,
};
use secrecy::SecretString;
use uuid::Uuid;

use super::error::AuthError;
use super::token::{generate_token, hash_token};
use super::user::{load_authenticated_user, AuthenticatedUser};

/// Sliding refresh granularity (§4): `last_seen_at`/`expires_at` are only
/// rewritten once an hour has passed since the last write, so a busy session
/// does not take a DB write per request.
pub const SLIDING_REFRESH_INTERVAL: Duration = Duration::hours(1);

/// Creates a new session for `user_id`: generates a token, stores
/// `sha256(token)`, and opportunistically prunes already-expired sessions in
/// the same call (§4: "pruned opportunistically on login"). Returns the raw
/// token — the only time it exists outside memory — for the caller to set as
/// the `fr_session` cookie.
pub async fn create_session(
    db: &DatabaseConnection,
    user_id: Uuid,
    ttl_days: i64,
    user_agent: Option<String>,
) -> Result<SecretString, AuthError> {
    prune_expired_sessions(db).await?;

    let token = generate_token();
    let now = Utc::now();
    let session = user_session::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id),
        token_hash: Set(hash_token(secrecy::ExposeSecret::expose_secret(&token))),
        created_at: Set(now.fixed_offset()),
        expires_at: Set((now + Duration::days(ttl_days)).fixed_offset()),
        last_seen_at: Set(now.fixed_offset()),
        user_agent: Set(user_agent),
    };
    session.insert(db).await?;

    Ok(token)
}

/// Verifies a raw session token: looks it up by `sha256(token)`, rejects a
/// missing/expired session or a disabled user, slides `last_seen_at`/
/// `expires_at` forward when the last refresh is over an hour old, and
/// returns the `AuthenticatedUser` the request context carries.
pub async fn verify_session(
    db: &DatabaseConnection,
    token: &str,
    ttl_days: i64,
) -> Result<AuthenticatedUser, AuthError> {
    let token_hash = hash_token(token);
    let session = user_session::Entity::find()
        .filter(user_session::Column::TokenHash.eq(token_hash))
        .one(db)
        .await?
        .ok_or(AuthError::SessionNotFound)?;

    let now = Utc::now();
    if session.expires_at.with_timezone(&Utc) <= now {
        return Err(AuthError::SessionNotFound);
    }

    let user = app_user::Entity::find_by_id(session.user_id)
        .one(db)
        .await?
        .ok_or(AuthError::SessionNotFound)?;
    if user.disabled {
        return Err(AuthError::UserDisabled);
    }

    if now - session.last_seen_at.with_timezone(&Utc) >= SLIDING_REFRESH_INTERVAL {
        let mut active: user_session::ActiveModel = session.clone().into();
        active.last_seen_at = Set(now.fixed_offset());
        active.expires_at = Set((now + Duration::days(ttl_days)).fixed_offset());
        active.update(db).await?;
    }

    load_authenticated_user(db, &user).await
}

/// Logout: deletes the session row matching `token`. Idempotent — revoking
/// an already-gone/unknown token is not an error, so logout always succeeds
/// from the caller's point of view.
pub async fn revoke_session(db: &DatabaseConnection, token: &str) -> Result<(), AuthError> {
    let token_hash = hash_token(token);
    if let Some(session) = user_session::Entity::find()
        .filter(user_session::Column::TokenHash.eq(token_hash))
        .one(db)
        .await?
    {
        session.delete(db).await?;
    }
    Ok(())
}

/// Deletes every session whose `expires_at` has passed. Called
/// opportunistically from [`create_session`]; also safe to call on its own
/// (e.g. a periodic sweep), returning the number of rows removed.
pub async fn prune_expired_sessions(db: &DatabaseConnection) -> Result<u64, AuthError> {
    let now: DateTime<Utc> = Utc::now();
    let result = user_session::Entity::delete_many()
        .filter(user_session::Column::ExpiresAt.lt(now.fixed_offset()))
        .exec(db)
        .await?;
    Ok(result.rows_affected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliding_refresh_interval_is_one_hour() {
        assert_eq!(SLIDING_REFRESH_INTERVAL, Duration::hours(1));
    }
}
