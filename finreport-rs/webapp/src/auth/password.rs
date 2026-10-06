//! Argon2id password hashing/verification (§4).
//!
//! Passwords are `SecretString` end-to-end — never a bare `String`, never
//! logged, never in a `Debug` impl. Only the PHC-formatted hash (not secret)
//! leaves this module.

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand_core::OsRng;
use secrecy::{ExposeSecret, SecretString};

use super::error::AuthError;

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
}
