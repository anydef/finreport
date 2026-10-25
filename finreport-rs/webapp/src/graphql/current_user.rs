//! The request's authenticated user (§4), and account-id scoping that every
//! resolver touching `account`/`account_balance`/`transaction` goes through —
//! no unscoped query path (§4/§5).

use async_graphql::{Context, ErrorExtensions, Guard, Result as GqlResult};
use entity::entities::app_user;
use sea_orm::{DatabaseConnection, EntityTrait};
use std::sync::Arc;
use uuid::Uuid;

pub use crate::auth::AuthenticatedUser;

/// The error every unauthenticated field but `me`/`login` returns:
/// `extensions.code = "UNAUTHENTICATED"` (§5), so the frontend can key its
/// redirect off the code rather than the message text.
pub fn unauthenticated_error() -> async_graphql::Error {
    async_graphql::Error::new("authentication required")
        .extend_with(|_, e| e.set("code", "UNAUTHENTICATED"))
}

/// Resolves the request's authenticated user, or `UNAUTHENTICATED`.
pub fn current_user<'ctx>(ctx: &Context<'ctx>) -> GqlResult<&'ctx AuthenticatedUser> {
    ctx.data::<Option<AuthenticatedUser>>()
        .ok()
        .and_then(|maybe| maybe.as_ref())
        .ok_or_else(unauthenticated_error)
}

/// `extensions.code = "FORBIDDEN"`: the caller is signed in but not allowed
/// to do this. Distinct from `UNAUTHENTICATED` so the frontend can show
/// "admin only" instead of bouncing the user to the login page.
pub fn forbidden_error() -> async_graphql::Error {
    async_graphql::Error::new("administrator access required")
        .extend_with(|_, e| e.set("code", "FORBIDDEN"))
}

/// Resolves the request's user and requires `app_user.is_admin`.
///
/// Authentication is exactly [`current_user`] (so an unauthenticated caller
/// still gets `UNAUTHENTICATED`, never `FORBIDDEN`). The flag is read from
/// the database on every call, never from the request and never from a
/// value cached on the session, so a demotion takes effect immediately. It
/// fails closed: a missing row, or a database error, is a refusal.
pub async fn require_admin<'ctx>(ctx: &Context<'ctx>) -> GqlResult<&'ctx AuthenticatedUser> {
    let user = current_user(ctx)?;
    let db: &DatabaseConnection = ctx.data::<Arc<DatabaseConnection>>()?;
    let is_admin = match app_user::Entity::find_by_id(user.user_id).one(db).await {
        Ok(row) => row.is_some_and(|row| row.is_admin && !row.disabled),
        Err(e) => {
            tracing::error!(error = %e, "could not read is_admin; refusing");
            return Err(async_graphql::Error::new("could not verify permissions"));
        }
    };
    if is_admin { Ok(user) } else { Err(forbidden_error()) }
}

/// Field guard for admin-only resolvers: put
/// `#[graphql(guard = "AdminOnly")]` on the field. It runs before the
/// resolver body, so the body cannot do any work (or start any write) for a
/// non-admin, and the requirement is visible on the field itself.
///
/// **The boundary rule - extend it the same way.** Shared state is
/// admin-only; the caller's own data is not.
/// - Admin: anything that changes (or exposes the configuration of) state
///   that every user's data is labelled by. Today that is the category
///   taxonomy (`category` has no effective owner), rules and learning
///   exemptions (neither has a `user_id`).
/// - Not admin: anything keyed or scoped to the caller - their accounts,
///   transactions, goals, savings flags, display aliases, Amazon orders.
///
/// A resolver that is both (say, a per-user view of a shared table) should
/// stay open and scope by the caller's accounts instead. Document the
/// requirement in the field's doc comment so it reaches the SDL.
pub struct AdminOnly;

impl Guard for AdminOnly {
    async fn check(&self, ctx: &Context<'_>) -> GqlResult<()> {
        require_admin(ctx).await.map(|_| ())
    }
}

/// Pure, DB-free helper that scopes a request down to the account ids the
/// given user is allowed to see.
///
/// - `requested` is `None` or empty -> all of `current_user.account_ids`.
/// - every id in `requested` is in `current_user.account_ids` -> exactly
///   those ids, in the order requested.
/// - any id in `requested` is **not** in `current_user.account_ids` -> `Err`
///   (not accessible) — a caller never silently loses an id it asked for
///   (§5).
pub fn scoped_account_ids(
    current_user: &AuthenticatedUser,
    requested: Option<&[Uuid]>,
) -> GqlResult<Vec<Uuid>> {
    match requested {
        None => Ok(current_user.account_ids.clone()),
        Some(ids) if ids.is_empty() => Ok(current_user.account_ids.clone()),
        Some(ids) => {
            for id in ids {
                if !current_user.account_ids.contains(id) {
                    return Err(async_graphql::Error::new(format!(
                        "account '{id}' is not accessible"
                    )));
                }
            }
            Ok(ids.to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(e: &async_graphql::Error) -> String {
        e.extensions
            .as_ref()
            .and_then(|x| x.get("code"))
            .map(|c| c.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn forbidden_and_unauthenticated_are_distinct_classified_errors() {
        assert_eq!(code_of(&forbidden_error()), "\"FORBIDDEN\"");
        assert_eq!(code_of(&unauthenticated_error()), "\"UNAUTHENTICATED\"");
    }

    fn uuid(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn user() -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: uuid(0),
            username: "default".to_string(),
            account_ids: vec![uuid(1), uuid(2)],
        }
    }

    #[test]
    fn none_requested_returns_all_account_ids() {
        let u = user();
        let result = scoped_account_ids(&u, None).expect("should succeed");
        assert_eq!(result, vec![uuid(1), uuid(2)]);
    }

    #[test]
    fn empty_requested_returns_all_account_ids() {
        let u = user();
        let result = scoped_account_ids(&u, Some(&[])).expect("should succeed");
        assert_eq!(result, vec![uuid(1), uuid(2)]);
    }

    #[test]
    fn requested_ids_present_returns_only_those_ids() {
        let u = user();
        let result = scoped_account_ids(&u, Some(&[uuid(2)])).expect("should succeed");
        assert_eq!(result, vec![uuid(2)]);
    }

    #[test]
    fn requested_id_absent_errors() {
        let u = user();
        let result = scoped_account_ids(&u, Some(&[uuid(3)]));
        assert!(result.is_err());
    }

    #[test]
    fn one_absent_id_among_present_ones_errors() {
        let u = user();
        let result = scoped_account_ids(&u, Some(&[uuid(1), uuid(3)]));
        assert!(result.is_err());
    }

    #[test]
    fn empty_account_ids_with_no_request_returns_empty() {
        let u = AuthenticatedUser {
            user_id: uuid(0),
            username: "default".to_string(),
            account_ids: vec![],
        };
        let result = scoped_account_ids(&u, None).expect("should succeed");
        assert!(result.is_empty());
    }
}
