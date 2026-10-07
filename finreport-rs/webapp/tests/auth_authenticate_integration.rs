//! `auth::authenticate` integration tests (§4), gated behind the
//! `integration` feature and requiring the throwaway `finreport-wp4-pg`
//! Postgres container (`127.0.0.1:55435`, see `tests/common/mod.rs`).
//! Covers both "invalid credentials" branches — unknown username and wrong
//! password for a real user — since the fix under test (a dummy Argon2id
//! verify for unknown usernames) only changes *how* the unknown-username
//! path gets to `InvalidCredentials`, not that it does.
#![cfg(feature = "integration")]

mod common;

use secrecy::SecretString;
use webapp::auth::{self, AuthError};

#[tokio::test]
async fn unknown_username_is_invalid_credentials_not_a_distinct_error() {
    let db = common::db().await;

    let err = auth::authenticate(
        &db,
        "no-such-user-at-all",
        &SecretString::from("whatever".to_string()),
    )
    .await
    .expect_err("an unknown username must not authenticate");

    assert!(
        matches!(err, AuthError::InvalidCredentials),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn wrong_password_for_a_real_user_is_the_same_invalid_credentials() {
    let db = common::db().await;
    let (_user_id, username) = common::seed_user(&db, "dave-auth-test", "s3cret-password").await;

    let err = auth::authenticate(
        &db,
        &username,
        &SecretString::from("definitely-wrong".to_string()),
    )
    .await
    .expect_err("a wrong password must not authenticate");

    assert!(
        matches!(err, AuthError::InvalidCredentials),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn correct_password_for_a_real_user_authenticates() {
    let db = common::db().await;
    let (user_id, username) = common::seed_user(&db, "erin-auth-test", "s3cret-password").await;

    let user = auth::authenticate(&db, &username, &SecretString::from("s3cret-password".to_string()))
        .await
        .expect("the right password must authenticate");

    assert_eq!(user.id, user_id);
}
