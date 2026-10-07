//! Argon2id password hashing/verification (§4).
//!
//! Passwords are `SecretString` end-to-end — never a bare `String`, never
//! logged, never in a `Debug` impl. Only the PHC-formatted hash (not secret)
//! leaves this module.

use std::sync::LazyLock;

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand_core::OsRng;
use secrecy::{ExposeSecret, SecretString};

use super::error::AuthError;

/// A fixed Argon2id PHC hash of an arbitrary placeholder password, never a
/// real credential. [`verify_password_async`] against this is how
/// [`super::user::authenticate`] spends the same CPU time on an unknown
/// username as it would checking a real one — without it, a request timer
/// could tell "no such user" apart from "wrong password" even though both
/// return [`AuthError::InvalidCredentials`] (§4: no username timing oracle).
static DUMMY_PASSWORD_HASH: LazyLock<String> = LazyLock::new(|| {
    hash_password(&SecretString::from(
        "dummy-password-for-constant-time-login".to_string(),
    ))
    .expect("hashing the fixed dummy password must succeed")
});

/// Hashes `password` with Argon2id, default `Params` (m=19456 KiB, t=2, p=1)
/// and a fresh random salt, returning the PHC string stored in
/// `app_user.password_hash`.
pub fn hash_password(password: &SecretString) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default().hash_password(password.expose_secret().as_bytes(), &salt)?;
    Ok(hash.to_string())
}

/// Verifies `password` against a PHC string previously produced by
/// [`hash_password`]. Returns `Ok(true)`/`Ok(false)` rather than threading a
/// wrong-password case through `AuthError`, so callers collapse it into
/// [`AuthError::InvalidCredentials`] themselves without this module knowing
/// about login semantics.
pub fn verify_password(password: &SecretString, phc: &str) -> Result<bool, AuthError> {
    let parsed_hash = PasswordHash::new(phc)?;
    match Argon2::default().verify_password(password.expose_secret().as_bytes(), &parsed_hash) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// [`hash_password`], off the async executor. Argon2id is deliberately
/// expensive (that's the point), so running it inline on a Tokio worker
/// thread would stall every other request being served by that thread;
/// `spawn_blocking` moves it to the blocking pool instead.
pub async fn hash_password_async(password: SecretString) -> Result<String, AuthError> {
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|e| AuthError::TaskJoin(e.to_string()))?
}

/// [`verify_password`], off the async executor — see [`hash_password_async`].
pub async fn verify_password_async(password: SecretString, phc: String) -> Result<bool, AuthError> {
    tokio::task::spawn_blocking(move || verify_password(&password, &phc))
        .await
        .map_err(|e| AuthError::TaskJoin(e.to_string()))?
}

/// Runs a dummy Argon2id verify against [`DUMMY_PASSWORD_HASH`] so the
/// "unknown username" branch of login costs the same as a real one. The
/// result is always `false` (nothing hashes to this PHC string by design);
/// only the timing matters, hence this never returns the `bool`.
pub async fn verify_dummy_password_async(password: SecretString) {
    let _ = verify_password_async(password, DUMMY_PASSWORD_HASH.clone()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(s: &str) -> SecretString {
        SecretString::from(s.to_string())
    }

    #[test]
    fn hash_then_verify_round_trips() {
        let password = secret("correct horse battery staple");
        let hash = hash_password(&password).expect("hashing should succeed");

        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password(&password, &hash).expect("verify should not error"));
    }

    #[test]
    fn wrong_password_is_rejected() {
        let hash = hash_password(&secret("correct horse battery staple")).unwrap();

        assert!(
            !verify_password(&secret("wrong password"), &hash).expect("verify should not error")
        );
    }

    #[test]
    fn same_password_hashes_differently_each_time() {
        // Random per-password salt (§4): two hashes of the same password must
        // never collide, yet both must still verify.
        let password = secret("correct horse battery staple");
        let a = hash_password(&password).unwrap();
        let b = hash_password(&password).unwrap();

        assert_ne!(a, b);
        assert!(verify_password(&password, &a).unwrap());
        assert!(verify_password(&password, &b).unwrap());
    }

    #[test]
    fn malformed_hash_is_a_hash_error_not_a_panic() {
        let err = verify_password(&secret("anything"), "not-a-phc-string").unwrap_err();
        assert!(matches!(err, AuthError::Hash(_)));
    }

    #[tokio::test]
    async fn async_variants_round_trip_through_spawn_blocking() {
        let password = secret("correct horse battery staple");
        let hash = hash_password_async(password.clone())
            .await
            .expect("async hashing should succeed");

        assert!(hash.starts_with("$argon2id$"));
        assert!(
            verify_password_async(password.clone(), hash.clone())
                .await
                .expect("async verify should not error")
        );
        assert!(
            !verify_password_async(secret("wrong password"), hash)
                .await
                .expect("async verify should not error")
        );
    }

    #[tokio::test]
    async fn dummy_verify_never_matches_and_never_panics() {
        // Nothing hashes to `DUMMY_PASSWORD_HASH` by construction — this is
        // purely exercising that the constant-time branch runs without
        // erroring, for arbitrary attacker-supplied input.
        verify_dummy_password_async(secret("anything an attacker might send")).await;
        verify_dummy_password_async(secret("")).await;
    }

    #[test]
    fn dummy_password_hash_is_a_valid_phc_string_nothing_else_matches() {
        assert!(DUMMY_PASSWORD_HASH.starts_with("$argon2id$"));
        assert!(!verify_password(&secret("password"), &DUMMY_PASSWORD_HASH).unwrap());
        assert!(!verify_password(&secret(""), &DUMMY_PASSWORD_HASH).unwrap());
    }
}
