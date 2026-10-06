//! User/account administration + the `AuthenticatedUser` the request context
//! carries (§4). Plain library functions — no actix, no async-graphql — so
//! both `user-admin` and WP4's GraphQL wiring call the same code.

use chrono::Utc;
use entity::entities::{account, app_user, user_account};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, ModelTrait, QueryFilter, Set,
    TransactionTrait,
};
use secrecy::SecretString;
use uuid::Uuid;

use super::error::AuthError;
use super::password::{hash_password, verify_password};

/// The identity + account scope injected into the GraphQL request context
/// (§4). `account_ids` comes from `user_account` in one query per request —
/// every resolver reading `account`/`account_balance`/`transaction` scopes to
/// this list, no unscoped query path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedUser {
    pub user_id: Uuid,
    pub username: String,
    pub account_ids: Vec<Uuid>,
}

/// Lowercases a username the way every lookup/insert in this module does
/// (§3: "`username` is lowercased by the application").
fn normalize_username(username: &str) -> String {
    username.to_lowercase()
}

/// Loads the `account_id`s linked to `user_id` via `user_account`, in the
/// stable `AuthenticatedUser.account_ids` shape.
async fn load_account_ids(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<Vec<Uuid>, AuthError> {
    let ids = user_account::Entity::find()
        .filter(user_account::Column::UserId.eq(user_id))
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.account_id)
        .collect();
    Ok(ids)
}

/// Builds the `AuthenticatedUser` for an already-resolved, already-verified
/// `app_user` row (caller has checked `disabled`).
pub async fn load_authenticated_user(
    db: &DatabaseConnection,
    user: &app_user::Model,
) -> Result<AuthenticatedUser, AuthError> {
    Ok(AuthenticatedUser {
        user_id: user.id,
        username: user.username.clone(),
        account_ids: load_account_ids(db, user.id).await?,
    })
}

/// Verifies `username`/`password` against `app_user` (login). Returns
/// [`AuthError::InvalidCredentials`] for both "no such user" and "wrong
/// password" (no username enumeration), and
/// [`AuthError::UserDisabled`] only once the password has actually matched —
/// a disabled account does not leak whether the password was right.
pub async fn authenticate(
    db: &DatabaseConnection,
    username: &str,
    password: &SecretString,
) -> Result<app_user::Model, AuthError> {
    let username = normalize_username(username);
    let user = app_user::Entity::find()
        .filter(app_user::Column::Username.eq(&username))
        .one(db)
        .await?
        .ok_or(AuthError::InvalidCredentials)?;

    if !verify_password(password, &user.password_hash)? {
        return Err(AuthError::InvalidCredentials);
    }
    if user.disabled {
        return Err(AuthError::UserDisabled);
    }

    Ok(user)
}

/// `user-admin create-user`: inserts a new `app_user` with a fresh Argon2id
/// hash. Fails with [`AuthError::UsernameTaken`] rather than surfacing the
/// raw unique-constraint violation.
pub async fn create_user(
    db: &DatabaseConnection,
    username: &str,
    password: &SecretString,
    display_name: Option<&str>,
) -> Result<app_user::Model, AuthError> {
    let username = normalize_username(username);
    if app_user::Entity::find()
        .filter(app_user::Column::Username.eq(&username))
        .one(db)
        .await?
        .is_some()
    {
        return Err(AuthError::UsernameTaken(username));
    }

    let model = app_user::ActiveModel {
        id: Set(Uuid::new_v4()),
        username: Set(username),
        password_hash: Set(hash_password(password)?),
        display_name: Set(display_name.map(str::to_string)),
        disabled: Set(false),
        created_at: Set(Utc::now().fixed_offset()),
    };
    Ok(model.insert(db).await?)
}

/// `user-admin set-password`: re-hashes and overwrites `password_hash` for an
/// existing user.
pub async fn set_password(
    db: &DatabaseConnection,
    username: &str,
    password: &SecretString,
) -> Result<(), AuthError> {
    let username = normalize_username(username);
    let user = app_user::Entity::find()
        .filter(app_user::Column::Username.eq(&username))
        .one(db)
        .await?
        .ok_or_else(|| AuthError::UnknownUser(username.clone()))?;

    let mut active: app_user::ActiveModel = user.into();
    active.password_hash = Set(hash_password(password)?);
    active.update(db).await?;
    Ok(())
}

/// `user-admin list-users`: all `app_user` rows, oldest first.
pub async fn list_users(db: &DatabaseConnection) -> Result<Vec<app_user::Model>, AuthError> {
    use sea_orm::QueryOrder;
    Ok(app_user::Entity::find()
        .order_by_asc(app_user::Column::CreatedAt)
        .all(db)
        .await?)
}

/// `user-admin list-accounts`: all `account` rows, for picking a
/// `source:external_id` to pass to `link`/`unlink`.
pub async fn list_accounts(db: &DatabaseConnection) -> Result<Vec<account::Model>, AuthError> {
    use sea_orm::QueryOrder;
    Ok(account::Entity::find()
        .order_by_asc(account::Column::FirstSeenAt)
        .all(db)
        .await?)
}

async fn find_user(db: &DatabaseConnection, username: &str) -> Result<app_user::Model, AuthError> {
    let username = normalize_username(username);
    app_user::Entity::find()
        .filter(app_user::Column::Username.eq(&username))
        .one(db)
        .await?
        .ok_or(AuthError::UnknownUser(username))
}

async fn find_account(
    db: &DatabaseConnection,
    source: &str,
    external_id: &str,
) -> Result<account::Model, AuthError> {
    account::Entity::find()
        .filter(account::Column::Source.eq(source))
        .filter(account::Column::ExternalId.eq(external_id))
        .one(db)
        .await?
        .ok_or_else(|| AuthError::UnknownAccount(source.to_string(), external_id.to_string()))
}

/// `user-admin link --account <source>:<external-id>`: links one account to
/// one user. Idempotent — linking an already-linked account is a no-op, not
/// an error (re-running `link` is safe).
pub async fn link_account(
    db: &DatabaseConnection,
    username: &str,
    source: &str,
    external_id: &str,
) -> Result<(), AuthError> {
    let user = find_user(db, username).await?;
    let account = find_account(db, source, external_id).await?;
    link_user_to_account(db, user.id, account.id).await
}

async fn link_user_to_account(
    db: &DatabaseConnection,
    user_id: Uuid,
    account_id: Uuid,
) -> Result<(), AuthError> {
    let already_linked = user_account::Entity::find_by_id((user_id, account_id))
        .one(db)
        .await?
        .is_some();
    if already_linked {
        return Ok(());
    }

    let link = user_account::ActiveModel {
        user_id: Set(user_id),
        account_id: Set(account_id),
        created_at: Set(Utc::now().fixed_offset()),
    };
    link.insert(db).await?;
    Ok(())
}

/// `user-admin link --all`: links every account not yet linked to this user.
/// Returns how many links were newly created (already-linked accounts are
/// skipped, not errors).
pub async fn link_all_accounts(db: &DatabaseConnection, username: &str) -> Result<usize, AuthError> {
    let user = find_user(db, username).await?;
    let txn = db.begin().await?;

    let accounts = account::Entity::find().all(&txn).await?;
    let mut linked = 0usize;
    for account in accounts {
        let already_linked = user_account::Entity::find_by_id((user.id, account.id))
            .one(&txn)
            .await?
            .is_some();
        if already_linked {
            continue;
        }
        let link = user_account::ActiveModel {
            user_id: Set(user.id),
            account_id: Set(account.id),
            created_at: Set(Utc::now().fixed_offset()),
        };
        link.insert(&txn).await?;
        linked += 1;
    }

    txn.commit().await?;
    Ok(linked)
}

/// `user-admin unlink --account <source>:<external-id>`: removes one
/// `user_account` row. Idempotent — unlinking an account that was never
/// linked is a no-op.
pub async fn unlink_account(
    db: &DatabaseConnection,
    username: &str,
    source: &str,
    external_id: &str,
) -> Result<(), AuthError> {
    let user = find_user(db, username).await?;
    let account = find_account(db, source, external_id).await?;

    if let Some(link) = user_account::Entity::find_by_id((user.id, account.id))
        .one(db)
        .await?
    {
        link.delete(db).await?;
    }
    Ok(())
}
