//! Temporary stand-in for WP2's auth core (§4).
//!
//! WP2 owns `webapp/src/auth/**` on a parallel branch and is not merged here
//! yet, so this module implements the exact functions/signatures §4
//! describes (Argon2id hash/verify, session token generation + SHA-256
//! storage, `AuthenticatedUser` loading) as **plain library functions** — no
//! actix, no async-graphql — so that swapping WP4's call sites to the real
//! `crate::auth` later is a one-line import change:
//!
//! ```ignore
//! use crate::graphql::auth_shim as auth; // temporary
//! use crate::auth;                       // after WP2 lands
//! ```
//!
//! Every other file in `graphql/` reaches these functions only through that
//! single `use` line (see `current_user.rs`, `mutations.rs`, `main.rs`).

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use entity::entities::prelude::{Account, AppUser, UserAccount, UserSession};
use entity::entities::{app_user, user_account, user_session};
use rand_core::{OsRng, RngCore};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter,
    QuerySelect, Set, TransactionTrait,
};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use uuid::Uuid;

use crate::graphql::current_user::AuthenticatedUser;

/// Sliding `last_seen_at` refresh window (§4): at most once per hour, so a
/// session that's being actively used doesn't cost a write per request.
const LAST_SEEN_REFRESH: Duration = Duration::hours(1);

#[derive(Debug)]
pub enum AuthError {
    /// Deliberately the same error for "no such user" and "wrong password" —
    /// no user enumeration (§5).
    InvalidCredentials,
    Db(DbErr),
}

impl From<DbErr> for AuthError {
    fn from(value: DbErr) -> Self {
        AuthError::Db(value)
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::InvalidCredentials => write!(f, "invalid username or password"),
            AuthError::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for AuthError {}

/// Argon2id with library defaults (m=19456 KiB, t=2, p=1), per §4.
fn argon2() -> Argon2<'static> {
    Argon2::default()
}

/// Hashes a password into a PHC string, for `user-admin` and test seeding.
pub fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    argon2()
        .hash_password(password.as_bytes(), &salt)
        .expect("argon2 hashing never fails for valid input")
        .to_string()
}

fn verify_password(password: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(hash) => argon2().verify_password(password.as_bytes(), &hash).is_ok(),
        Err(_) => false,
    }
}

/// A fixed, lazily-computed PHC string hashed from a value nobody will ever
/// type, verified against on an unknown username so an unknown-user login
/// takes the same code path (and roughly the same time) as a wrong-password
/// one — no user enumeration (§5).
fn dummy_phc() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| hash_password("not-a-real-password-this-will-never-match"))
}

fn sha256_hex_bytes(raw: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hasher.finalize().to_vec()
}

/// 32 random bytes, base64url-encoded (§4) — this string is the cookie
/// value and is never itself stored.
fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

async fn account_ids_for_user(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<Vec<Uuid>, DbErr> {
    UserAccount::find()
        .filter(user_account::Column::UserId.eq(user_id))
        .select_only()
        .column(user_account::Column::AccountId)
        .into_tuple::<Uuid>()
        .all(db)
        .await
}

/// Verifies `username`/`password`, creates a session row and returns the
/// loaded user plus the raw cookie token. Same latency and same error for an
/// unknown username and a wrong password (§5).
pub async fn authenticate(
    db: &DatabaseConnection,
    username: &str,
    password: &SecretString,
    session_ttl_days: i64,
    user_agent: Option<String>,
) -> Result<(AuthenticatedUser, String), AuthError> {
    let username_lc = username.to_lowercase();
    let user = AppUser::find()
        .filter(app_user::Column::Username.eq(&username_lc))
        .one(db)
        .await?;

    let user = match user {
        Some(u) if !u.disabled => {
            if !verify_password(password.expose_secret(), &u.password_hash) {
                return Err(AuthError::InvalidCredentials);
            }
            u
        }
        Some(_disabled) => {
            // Still run a verify against the dummy hash so a disabled
            // account doesn't respond measurably faster than a wrong
            // password on an enabled one.
            verify_password(password.expose_secret(), dummy_phc());
            return Err(AuthError::InvalidCredentials);
        }
        None => {
            verify_password(password.expose_secret(), dummy_phc());
            return Err(AuthError::InvalidCredentials);
        }
    };

    let raw_token = generate_token();
    let token_hash = sha256_hex_bytes(&raw_token);
    let now = Utc::now();
    let session = user_session::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user.id),
        token_hash: Set(token_hash),
        created_at: Set(now.into()),
        expires_at: Set((now + Duration::days(session_ttl_days)).into()),
        last_seen_at: Set(now.into()),
        user_agent: Set(user_agent),
    };
    session.insert(db).await?;

    let account_ids = account_ids_for_user(db, user.id).await?;
    Ok((
        AuthenticatedUser {
            user_id: user.id,
            username: user.username,
            display_name: user.display_name,
            account_ids,
        },
        raw_token,
    ))
}

/// Resolves a raw cookie token to its session's user, rejecting expired
/// sessions and disabled users, and refreshing `last_seen_at` at most once
/// per hour (§4).
pub async fn session_user(
    db: &DatabaseConnection,
    raw_token: &str,
) -> Result<Option<AuthenticatedUser>, DbErr> {
    let token_hash = sha256_hex_bytes(raw_token);
    let now = Utc::now();

    let Some((session, Some(user))) = UserSession::find()
        .filter(user_session::Column::TokenHash.eq(token_hash))
        .find_also_related(AppUser)
        .one(db)
        .await?
    else {
        return Ok(None);
    };

    if user.disabled {
        return Ok(None);
    }
    let expires_at: DateTime<Utc> = session.expires_at.into();
    if expires_at <= now {
        return Ok(None);
    }

    let last_seen_at: DateTime<Utc> = session.last_seen_at.into();
    if now - last_seen_at >= LAST_SEEN_REFRESH {
        let mut active: user_session::ActiveModel = session.into();
        active.last_seen_at = Set(now.into());
        active.update(db).await?;
    }

    let account_ids = account_ids_for_user(db, user.id).await?;
    Ok(Some(AuthenticatedUser {
        user_id: user.id,
        username: user.username,
        display_name: user.display_name,
        account_ids,
    }))
}

/// Deletes the session row for `raw_token`, if any. Idempotent — logging out
/// twice, or with a stale cookie, is not an error.
pub async fn revoke_session(db: &DatabaseConnection, raw_token: &str) -> Result<(), DbErr> {
    let token_hash = sha256_hex_bytes(raw_token);
    UserSession::delete_many()
        .filter(user_session::Column::TokenHash.eq(token_hash))
        .exec(db)
        .await?;
    Ok(())
}

/// Builds the `Set-Cookie` header value for a fresh session (§4): `fr_session`,
/// `HttpOnly`, `SameSite=Lax`, `Path=/`, `Max-Age` = TTL, `Secure` controlled
/// by `APP_cookie_secure`.
pub fn cookie_header(raw_token: &str, ttl_days: i64, secure: bool) -> String {
    let max_age = ttl_days * 24 * 60 * 60;
    let secure_flag = if secure { "; Secure" } else { "" };
    format!("fr_session={raw_token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure_flag}")
}

/// Builds the `Set-Cookie` header value that clears the session cookie on
/// logout.
pub fn clear_cookie_header(secure: bool) -> String {
    let secure_flag = if secure { "; Secure" } else { "" };
    format!("fr_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{secure_flag}")
}

/// Reads `fr_session` out of a raw `Cookie` request header value.
pub fn extract_token(cookie_header: &str) -> Option<String> {
    cookie_header.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix("fr_session=").map(|v| v.to_string())
    })
}

/// Loads the `account.id`s for the accounts an `app_user` can see — used by
/// test seeding (`link`, in spirit of `user-admin link`), since `user-admin`
/// itself is WP2's.
pub async fn link_account(
    db: &DatabaseConnection,
    user_id: Uuid,
    account_id: Uuid,
) -> Result<(), DbErr> {
    db.transaction::<_, (), DbErr>(|txn| {
        Box::pin(async move {
            if Account::find_by_id(account_id).one(txn).await?.is_none() {
                return Err(DbErr::RecordNotFound(format!("account {account_id}")));
            }
            let link = user_account::ActiveModel {
                user_id: Set(user_id),
                account_id: Set(account_id),
                created_at: Set(Utc::now().into()),
            };
            link.insert(txn).await?;
            Ok(())
        })
    })
    .await
    .map_err(|e| match e {
        sea_orm::TransactionError::Connection(e) => e,
        sea_orm::TransactionError::Transaction(e) => e,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_round_trip() {
        let hash = hash_password("hunter2-correct-horse");
        assert!(verify_password("hunter2-correct-horse", &hash));
    }

    #[test]
    fn wrong_password_is_rejected() {
        let hash = hash_password("hunter2-correct-horse");
        assert!(!verify_password("wrong-password", &hash));
    }

    #[test]
    fn dummy_hash_never_verifies_against_a_real_password() {
        assert!(!verify_password("hunter2-correct-horse", dummy_phc()));
    }

    #[test]
    fn token_hashing_is_deterministic_and_distinct_per_token() {
        let a = sha256_hex_bytes("token-a");
        let b = sha256_hex_bytes("token-a");
        let c = sha256_hex_bytes("token-b");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn generated_tokens_are_unique_and_url_safe() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn cookie_header_carries_the_configured_ttl_and_secure_flag() {
        let header = cookie_header("tok", 30, true);
        assert!(header.contains("fr_session=tok"));
        assert!(header.contains("Max-Age=2592000"));
        assert!(header.contains("Secure"));
        assert!(header.contains("HttpOnly"));
        assert!(header.contains("SameSite=Lax"));

        let insecure = cookie_header("tok", 30, false);
        assert!(!insecure.contains("Secure"));
    }

    #[test]
    fn extract_token_reads_the_cookie_among_others() {
        assert_eq!(
            extract_token("foo=bar; fr_session=abc123; other=1"),
            Some("abc123".to_string())
        );
        assert_eq!(extract_token("foo=bar"), None);
    }
}
