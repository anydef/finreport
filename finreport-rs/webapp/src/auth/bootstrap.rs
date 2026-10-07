//! Admin bootstrap, run once at `webapp` startup after DB connect (§3/§4):
//! if `APP_admin_password` is set, ensures `APP_admin_username` exists,
//! is marked `is_admin`, and its password hash matches, then links every
//! account to it — reusing the same `auth::user` helpers `user-admin` drives
//! from the CLI, so bootstrap and manual administration never diverge.
//!
//! Idempotent by design: it runs on every `webapp` start/restart, including
//! after a Terraform-driven password rotation (`random_password.admin`), so
//! re-running it must never create a duplicate user or fail on an
//! already-linked account.

use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use secrecy::SecretString;
use tracing::info;

use entity::entities::app_user;

use super::error::AuthError;
use super::password::{hash_password_async, verify_password_async};
use super::user::{create_user, link_all_accounts};

/// Entry point called from `webapp::main` right after the DB connection is
/// established. A `None` password is a deliberate no-op — local dev
/// (`just dev-be`) never sets `APP_admin_password`, so no admin user is
/// created there.
pub async fn bootstrap_admin(
    db: &DatabaseConnection,
    username: &str,
    password: Option<&SecretString>,
) -> Result<(), AuthError> {
    let Some(password) = password else {
        info!("[startup] APP_admin_password unset; skipping admin bootstrap");
        return Ok(());
    };

    let existing = app_user::Entity::find()
        .filter(app_user::Column::Username.eq(username))
        .one(db)
        .await?;

    match existing {
        None => {
            create_user(db, username, password, Some("Admin")).await?;
            // `create_user` always inserts `is_admin = false` (it is shared
            // with `user-admin create-user`, which never creates admins) —
            // promote the row it just made.
            promote_to_admin(db, username).await?;
            info!(%username, "[startup] created admin user");
        }
        Some(user) => {
            let password_matches =
                verify_password_async(password.clone(), user.password_hash.clone()).await?;
            let needs_promotion = !user.is_admin;

            if !password_matches || needs_promotion {
                let mut active: app_user::ActiveModel = user.into();
                if !password_matches {
                    active.password_hash = Set(hash_password_async(password.clone()).await?);
                }
                if needs_promotion {
                    active.is_admin = Set(true);
                }
                active.update(db).await?;
                info!(
                    %username,
                    rotated_password = !password_matches,
                    promoted = needs_promotion,
                    "[startup] admin user updated"
                );
            } else {
                info!(%username, "[startup] admin user already up to date");
            }
        }
    }

    let linked = link_all_accounts(db, username).await?;
    info!(%username, linked, "[startup] linked accounts to admin");

    Ok(())
}

/// Sets `is_admin = true` on a freshly created row. Split out of
/// `create_user` (shared with `user-admin create-user`, which must never
/// create an admin implicitly) rather than threading an `is_admin` flag
/// through it.
async fn promote_to_admin(db: &DatabaseConnection, username: &str) -> Result<(), AuthError> {
    let user = app_user::Entity::find()
        .filter(app_user::Column::Username.eq(username))
        .one(db)
        .await?
        .ok_or_else(|| AuthError::UnknownUser(username.to_string()))?;
    let mut active: app_user::ActiveModel = user.into();
    active.is_admin = Set(true);
    active.update(db).await?;
    Ok(())
}

#[cfg(all(test, feature = "integration"))]
mod integration_tests {
    use chrono::Utc;
    use entity::entities::{account, app_user, user_account};
    use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
    use secrecy::SecretString;
    use testcontainers_modules::postgres::Postgres;
    use testcontainers_modules::testcontainers::ContainerAsync;
    use testcontainers_modules::testcontainers::runners::AsyncRunner;
    use uuid::Uuid;

    use super::bootstrap_admin;
    use crate::auth::password::verify_password;
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
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("failed to get mapped port");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
        let conn = init_db(&url).await.expect("migrations should apply cleanly");
        TestDb {
            _container: container,
            conn,
        }
    }

    async fn make_account(db: &DatabaseConnection, external_id: &str) -> account::Model {
        let now = Utc::now().fixed_offset();
        account::ActiveModel {
            id: Set(Uuid::new_v4()),
            source: Set("comdirect".to_string()),
            external_id: Set(external_id.to_string()),
            display_id: Set(None),
            account_type: Set(None),
            iban: Set(None),
            bic: Set(None),
            institute: Set(None),
            label: Set(None),
            currency: Set("EUR".to_string()),
            raw_payload: Set(None),
            origin: Set("comdirect".to_string()),
            first_seen_at: Set(now),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .expect("account insert should succeed")
    }

    fn secret(s: &str) -> SecretString {
        SecretString::from(s.to_string())
    }

    #[tokio::test]
    async fn creates_admin_when_missing() {
        let db = test_db().await;
        bootstrap_admin(&db.conn, "admin", Some(&secret("hunter2hunter2")))
            .await
            .expect("bootstrap should succeed");

        let user = app_user::Entity::find()
            .filter(app_user::Column::Username.eq("admin"))
            .one(&db.conn)
            .await
            .unwrap()
            .expect("admin user should exist");
        assert!(user.is_admin);
        assert!(verify_password(&secret("hunter2hunter2"), &user.password_hash).unwrap());
    }

    #[tokio::test]
    async fn rerun_is_idempotent() {
        let db = test_db().await;
        bootstrap_admin(&db.conn, "admin", Some(&secret("hunter2hunter2")))
            .await
            .unwrap();
        bootstrap_admin(&db.conn, "admin", Some(&secret("hunter2hunter2")))
            .await
            .unwrap();

        let count = app_user::Entity::find()
            .filter(app_user::Column::Username.eq("admin"))
            .all(&db.conn)
            .await
            .unwrap()
            .len();
        assert_eq!(count, 1, "bootstrap must not create a duplicate admin");
    }

    #[tokio::test]
    async fn rotates_password_when_it_no_longer_verifies() {
        let db = test_db().await;
        bootstrap_admin(&db.conn, "admin", Some(&secret("original-password")))
            .await
            .unwrap();
        bootstrap_admin(&db.conn, "admin", Some(&secret("rotated-password")))
            .await
            .unwrap();

        let user = app_user::Entity::find()
            .filter(app_user::Column::Username.eq("admin"))
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert!(verify_password(&secret("rotated-password"), &user.password_hash).unwrap());
        assert!(!verify_password(&secret("original-password"), &user.password_hash).unwrap());
    }

    #[tokio::test]
    async fn promotes_an_existing_non_admin_user() {
        let db = test_db().await;
        crate::auth::user::create_user(&db.conn, "admin", &secret("hunter2hunter2"), None)
            .await
            .expect("plain create_user should succeed");

        bootstrap_admin(&db.conn, "admin", Some(&secret("hunter2hunter2")))
            .await
            .unwrap();

        let user = app_user::Entity::find()
            .filter(app_user::Column::Username.eq("admin"))
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert!(user.is_admin);
    }

    #[tokio::test]
    async fn links_every_account_to_admin() {
        let db = test_db().await;
        let acc1 = make_account(&db.conn, "111").await;
        let acc2 = make_account(&db.conn, "222").await;

        bootstrap_admin(&db.conn, "admin", Some(&secret("hunter2hunter2")))
            .await
            .unwrap();

        let user = app_user::Entity::find()
            .filter(app_user::Column::Username.eq("admin"))
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        let linked: Vec<Uuid> = user_account::Entity::find()
            .filter(user_account::Column::UserId.eq(user.id))
            .all(&db.conn)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.account_id)
            .collect();
        assert!(linked.contains(&acc1.id));
        assert!(linked.contains(&acc2.id));

        // A later-projected account also gets linked on the next bootstrap
        // run, same as `user-admin link --all`.
        let acc3 = make_account(&db.conn, "333").await;
        bootstrap_admin(&db.conn, "admin", Some(&secret("hunter2hunter2")))
            .await
            .unwrap();
        let linked_after: Vec<Uuid> = user_account::Entity::find()
            .filter(user_account::Column::UserId.eq(user.id))
            .all(&db.conn)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.account_id)
            .collect();
        assert!(linked_after.contains(&acc3.id));
    }
}
