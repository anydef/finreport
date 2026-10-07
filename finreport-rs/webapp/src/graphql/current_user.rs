//! The request's authenticated user (§4), and account-id scoping that every
//! resolver touching `account`/`account_balance`/`transaction` goes through —
//! no unscoped query path (§4/§5).

use async_graphql::{Context, ErrorExtensions, Result as GqlResult};
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
