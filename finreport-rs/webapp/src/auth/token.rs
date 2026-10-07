//! Session token generation + SHA-256 hashing (§4).
//!
//! The raw token is the cookie value and is **never stored**; Postgres keeps
//! only `sha256(token)` in `user_session.token_hash`. Generation produces a
//! `SecretString` (it is as sensitive as a password while it exists in
//! memory); hashing takes a plain `&str` because the resulting digest is not
//! secret — it is a database lookup key, not a credential by itself.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand_core::{OsRng, RngCore};
use secrecy::SecretString;
use sha2::{Digest, Sha256};

/// Raw random bytes behind each session token before base64url-encoding.
pub const TOKEN_BYTES: usize = 32;

/// Generates a new session token: 32 random bytes from `OsRng`, base64url
/// (unpadded) encoded. This is the value set in the `fr_session` cookie.
pub fn generate_token() -> SecretString {
    let mut bytes = [0u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    SecretString::from(URL_SAFE_NO_PAD.encode(bytes))
}

/// `sha256(token)`, the value persisted in `user_session.token_hash`.
pub fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[test]
    fn generated_tokens_are_unique_and_url_safe() {
        let a = generate_token();
        let b = generate_token();

        assert_ne!(a.expose_secret(), b.expose_secret());
        // base64url alphabet, no padding.
        assert!(
            a.expose_secret()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert!(!a.expose_secret().contains('='));
    }

    #[test]
    fn hashing_is_deterministic_and_sha256_sized() {
        let hash_a = hash_token("same-token");
        let hash_b = hash_token("same-token");

        assert_eq!(hash_a, hash_b);
        assert_eq!(hash_a.len(), 32); // SHA-256 digest length.
    }

    #[test]
    fn different_tokens_hash_differently() {
        assert_ne!(hash_token("token-a"), hash_token("token-b"));
    }
}
