//! DB-backed tests for `user-admin`'s admin-flag operations: `create_user`
//! stays unprivileged, `set_admin` promotes/demotes idempotently and fails
//! cleanly on an unknown user. Gated behind the `integration` feature.

#![cfg(all(test, feature = "integration"))]

use entity::entities::app_user;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use secrecy::SecretString;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

use crate::auth::AuthError;
use crate::auth::user::{create_user, set_admin};
use crate::db::seaql::init_db;

struct TestDb {
    _container: ContainerAsync<Postgres>,
    conn: DatabaseConnection,
}

async fn test_db() -> TestDb {
    let container = Postgres::default()
        .start()
        .await
        .expect("failed to start throwaway Postgres container");
    let port = container.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let conn = init_db(&url).await.expect("migrations should apply");
    TestDb {
        _container: container,
        conn,
    }
}

async fn is_admin(db: &DatabaseConnection, username: &str) -> bool {
    app_user::Entity::find()
        .filter(app_user::Column::Username.eq(username))
        .one(db)
        .await
        .unwrap()
        .expect("user exists")
        .is_admin
}

fn pw() -> SecretString {
    SecretString::from("hunter2hunter2".to_string())
}

#[tokio::test]
async fn plain_create_user_is_not_admin() {
    let db = test_db().await;
    create_user(&db.conn, "alice", &pw(), None).await.unwrap();
    assert!(!is_admin(&db.conn, "alice").await);
}

#[tokio::test]
async fn create_then_set_admin_is_admin() {
    // The `create-user --admin` path: create, then set_admin(true).
    let db = test_db().await;
    create_user(&db.conn, "dev", &pw(), None).await.unwrap();
    assert!(set_admin(&db.conn, "dev", true).await.unwrap());
    assert!(is_admin(&db.conn, "dev").await);
}

#[tokio::test]
async fn promote_demote_round_trip_and_idempotent() {
    let db = test_db().await;
    create_user(&db.conn, "alice", &pw(), None).await.unwrap();

    assert!(set_admin(&db.conn, "alice", true).await.unwrap(), "changed");
    assert!(!set_admin(&db.conn, "alice", true).await.unwrap(), "repeat is a no-op");
    assert!(is_admin(&db.conn, "alice").await);

    assert!(set_admin(&db.conn, "alice", false).await.unwrap(), "changed");
    assert!(!set_admin(&db.conn, "alice", false).await.unwrap(), "repeat is a no-op");
    assert!(!is_admin(&db.conn, "alice").await);
}

#[tokio::test]
async fn demoting_the_only_admin_is_allowed() {
    let db = test_db().await;
    create_user(&db.conn, "solo", &pw(), None).await.unwrap();
    set_admin(&db.conn, "solo", true).await.unwrap();
    set_admin(&db.conn, "solo", false).await.unwrap();
    assert!(!is_admin(&db.conn, "solo").await);
}

#[tokio::test]
async fn set_admin_on_unknown_user_is_a_clear_error() {
    let db = test_db().await;
    match set_admin(&db.conn, "ghost", true).await {
        Err(AuthError::UnknownUser(u)) => assert_eq!(u, "ghost"),
        other => panic!("expected UnknownUser, got {other:?}"),
    }
}
