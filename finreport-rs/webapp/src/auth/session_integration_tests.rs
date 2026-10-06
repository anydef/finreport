//! DB-backed session-lifecycle tests (§4/WP2 "Done when").
//!
//! Gated behind the `integration` feature (`just test-integration`) — `just
//! test` / `cargo test --workspace` never enables it, so these stay
//! Docker-free by default. Each test spins up its own disposable Postgres via
//! `testcontainers`, runs the real migrations, and exercises the public
//! `auth` API exactly as the GraphQL layer and `user-admin` will.

#![cfg(all(test, feature = "integration"))]

use chrono::{Duration, Utc};
use entity::entities::{app_user, user_session};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use secrecy::{ExposeSecret, SecretString};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use uuid::Uuid;

use crate::auth::AuthError;
use crate::auth::session::{
    create_session, prune_expired_sessions, revoke_session, verify_session,
};
use crate::auth::user::create_user;
use crate::db::seaql::init_db;

/// Keeps the container alive for the test's duration — `testcontainers` tears
/// it down the moment this is dropped, so it must be held, not discarded.
struct TestDb {
    _container: ContainerAsync<Postgres>,
    conn: DatabaseConnection,
}

async fn test_db() -> TestDb {
    let container = Postgres::default()
        .start()
        .await
        .expect("failed to start throwaway Postgres container");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("failed to get mapped port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

    let conn = init_db(&url)
        .await
        .expect("migrations should apply cleanly");
    TestDb {
        _container: container,
        conn,
    }
}

async fn make_user(db: &DatabaseConnection, username: &str) -> app_user::Model {
    create_user(
        db,
        username,
        &SecretString::from("irrelevant-for-session-tests".to_string()),
        None,
    )
    .await
    .expect("user creation should succeed")
}

#[tokio::test]
async fn create_then_verify_round_trips_to_the_same_user() {
    let db = test_db().await;
    let user = make_user(&db.conn, "alice").await;

    let token = create_session(&db.conn, user.id, 30, Some("test-agent".to_string()))
        .await
        .expect("session creation should succeed");

    let authenticated = verify_session(&db.conn, token.expose_secret(), 30)
        .await
        .expect("verification should succeed");

    assert_eq!(authenticated.user_id, user.id);
    assert_eq!(authenticated.username, "alice");
    assert_eq!(authenticated.account_ids, Vec::<Uuid>::new());
}

#[tokio::test]
async fn unknown_token_is_rejected() {
    let db = test_db().await;

    let err = verify_session(&db.conn, "this-token-was-never-issued", 30)
        .await
        .unwrap_err();

    assert!(matches!(err, AuthError::SessionNotFound));
}

#[tokio::test]
async fn revoke_deletes_the_session_so_it_no_longer_verifies() {
    let db = test_db().await;
    let user = make_user(&db.conn, "bob").await;
    let token = create_session(&db.conn, user.id, 30, None).await.unwrap();

    revoke_session(&db.conn, token.expose_secret())
        .await
        .expect("revoke should succeed");

    let err = verify_session(&db.conn, token.expose_secret(), 30)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::SessionNotFound));
}

#[tokio::test]
async fn revoking_an_unknown_token_is_not_an_error() {
    let db = test_db().await;
    // Logout must always succeed from the caller's point of view, even for a
    // token that was never valid (already logged out, bad cookie, ...).
    revoke_session(&db.conn, "never-issued")
        .await
        .expect("revoke should be idempotent");
}

#[tokio::test]
async fn disabled_user_is_rejected_even_with_a_live_session() {
    let db = test_db().await;
    let user = make_user(&db.conn, "carol").await;
    let token = create_session(&db.conn, user.id, 30, None).await.unwrap();

    let mut active: app_user::ActiveModel = user.into();
    active.disabled = Set(true);
    active.update(&db.conn).await.unwrap();

    let err = verify_session(&db.conn, token.expose_secret(), 30)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::UserDisabled));
}

/// Directly rewrites the session row's `expires_at`/`last_seen_at` rather
/// than waiting out a TTL, to test the exact boundary deterministically.
async fn set_session_times(
    db: &DatabaseConnection,
    token: &str,
    expires_at: chrono::DateTime<Utc>,
    last_seen_at: chrono::DateTime<Utc>,
) {
    let token_hash = crate::auth::token::hash_token(token);
    let session = user_session::Entity::find()
        .filter(user_session::Column::TokenHash.eq(token_hash))
        .one(db)
        .await
        .unwrap()
        .expect("session row should exist");

    let mut active: user_session::ActiveModel = session.into();
    active.expires_at = Set(expires_at.fixed_offset());
    active.last_seen_at = Set(last_seen_at.fixed_offset());
    active.update(db).await.unwrap();
}

#[tokio::test]
async fn expiry_boundary_is_exclusive() {
    let db = test_db().await;
    let user = make_user(&db.conn, "dave").await;
    let token = create_session(&db.conn, user.id, 30, None).await.unwrap();
    let raw = token.expose_secret().to_string();

    let now = Utc::now();
    // One second in the future: still valid.
    set_session_times(&db.conn, &raw, now + Duration::seconds(1), now).await;
    verify_session(&db.conn, &raw, 30)
        .await
        .expect("a session expiring in the future should verify");

    // Exactly now (and in the past): expired, `expires_at <= now` rejects it.
    let now2 = Utc::now();
    set_session_times(&db.conn, &raw, now2, now2).await;
    let err = verify_session(&db.conn, &raw, 30).await.unwrap_err();
    assert!(matches!(err, AuthError::SessionNotFound));
}

#[tokio::test]
async fn sliding_refresh_extends_expiry_once_the_hour_has_passed() {
    let db = test_db().await;
    let user = make_user(&db.conn, "erin").await;
    let token = create_session(&db.conn, user.id, 30, None).await.unwrap();
    let raw = token.expose_secret().to_string();

    // Make the session look like it was last refreshed 2 hours ago, with an
    // expiry that would otherwise be imminent.
    let now = Utc::now();
    let stale_last_seen = now - Duration::hours(2);
    let soon = now + Duration::minutes(5);
    set_session_times(&db.conn, &raw, soon, stale_last_seen).await;

    verify_session(&db.conn, &raw, 30)
        .await
        .expect("verification should succeed and slide the TTL forward");

    let token_hash = crate::auth::token::hash_token(&raw);
    let refreshed = user_session::Entity::find()
        .filter(user_session::Column::TokenHash.eq(token_hash))
        .one(&db.conn)
        .await
        .unwrap()
        .expect("session should still exist");

    // expires_at should have been pushed out ~30 days from now, far beyond
    // the `soon` value it had before verification.
    assert!(refreshed.expires_at.with_timezone(&Utc) > now + Duration::days(29));
    assert!(refreshed.last_seen_at.with_timezone(&Utc) > stale_last_seen);
}

#[tokio::test]
async fn recent_refresh_is_not_rewritten() {
    let db = test_db().await;
    let user = make_user(&db.conn, "frank").await;
    let token = create_session(&db.conn, user.id, 30, None).await.unwrap();
    let raw = token.expose_secret().to_string();

    let now = Utc::now();
    let recent_last_seen = now - Duration::minutes(10);
    let original_expiry = now + Duration::days(30);
    set_session_times(&db.conn, &raw, original_expiry, recent_last_seen).await;

    verify_session(&db.conn, &raw, 30).await.unwrap();

    let token_hash = crate::auth::token::hash_token(&raw);
    let unchanged = user_session::Entity::find()
        .filter(user_session::Column::TokenHash.eq(token_hash))
        .one(&db.conn)
        .await
        .unwrap()
        .expect("session should still exist");

    // last_seen_at under an hour old: verify_session must not have written a
    // refresh (no write per request, §4).
    let delta = (unchanged.last_seen_at.with_timezone(&Utc) - recent_last_seen)
        .num_seconds()
        .abs();
    assert!(
        delta < 2,
        "last_seen_at should not have moved, delta={delta}s"
    );
}

#[tokio::test]
async fn prune_removes_only_expired_sessions() {
    let db = test_db().await;
    let user = make_user(&db.conn, "grace").await;

    let live = create_session(&db.conn, user.id, 30, None).await.unwrap();
    let expired = create_session(&db.conn, user.id, 30, None).await.unwrap();
    let now = Utc::now();
    set_session_times(
        &db.conn,
        expired.expose_secret(),
        now - Duration::days(1),
        now,
    )
    .await;

    let removed = prune_expired_sessions(&db.conn).await.unwrap();
    assert_eq!(removed, 1);

    assert!(
        verify_session(&db.conn, live.expose_secret(), 30)
            .await
            .is_ok()
    );

    let expired_hash = crate::auth::token::hash_token(expired.expose_secret());
    let gone = user_session::Entity::find()
        .filter(user_session::Column::TokenHash.eq(expired_hash))
        .one(&db.conn)
        .await
        .unwrap();
    assert!(gone.is_none());
}

#[tokio::test]
async fn account_ids_reflect_user_account_links() {
    let db = test_db().await;
    let user = make_user(&db.conn, "heidi").await;

    // No linked accounts yet: AuthenticatedUser.account_ids is empty, not an
    // error.
    let token = create_session(&db.conn, user.id, 30, None).await.unwrap();
    let authenticated = verify_session(&db.conn, token.expose_secret(), 30)
        .await
        .unwrap();
    assert!(authenticated.account_ids.is_empty());
}
