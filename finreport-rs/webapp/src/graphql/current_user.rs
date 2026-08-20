use entity::entities::prelude::Account as AccountEntity;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};

/// A lightweight placeholder for a future multitenancy/auth system.
///
/// The app currently has no login system: there is exactly one implicit
/// user who can see every account in the database. Every resolver that
/// touches accounts/transactions should go through [`current_user`] and
/// [`scoped_account_ids`] instead of querying "all accounts" directly, so
/// that a real auth system can later replace just this module.
#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub id: String,
    /// `Account.account_id` values this user may see.
    pub account_ids: Vec<String>,
}

/// Resolves the current user. For now this is always the single implicit
/// "default" user, who may see every account in the database.
pub async fn current_user(db: &DatabaseConnection) -> Result<CurrentUser, DbErr> {
    let accounts = AccountEntity::find().all(db).await?;
    Ok(CurrentUser {
        id: "default".to_string(),
        account_ids: accounts.into_iter().map(|a| a.account_id).collect(),
    })
}

/// Pure, DB-free helper that scopes a request down to the account ids the
/// given user is allowed to see.
///
/// - `requested = None` -> all of `current_user.account_ids`.
/// - `requested = Some(id)` and `id` is in `current_user.account_ids` -> `Ok(vec![id])`.
/// - `requested = Some(id)` and `id` is NOT in `current_user.account_ids` -> `Err` (not accessible).
pub fn scoped_account_ids(
    current_user: &CurrentUser,
    requested: Option<&str>,
) -> async_graphql::Result<Vec<String>> {
    match requested {
        None => Ok(current_user.account_ids.clone()),
        Some(id) => {
            if current_user.account_ids.iter().any(|a| a == id) {
                Ok(vec![id.to_string()])
            } else {
                Err(async_graphql::Error::new(format!(
                    "account '{id}' is not accessible"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> CurrentUser {
        CurrentUser {
            id: "default".to_string(),
            account_ids: vec!["acc-1".to_string(), "acc-2".to_string()],
        }
    }

    #[test]
    fn none_requested_returns_all_account_ids() {
        let u = user();
        let result = scoped_account_ids(&u, None).expect("should succeed");
        assert_eq!(result, vec!["acc-1".to_string(), "acc-2".to_string()]);
    }

    #[test]
    fn requested_id_present_returns_only_that_id() {
        let u = user();
        let result = scoped_account_ids(&u, Some("acc-2")).expect("should succeed");
        assert_eq!(result, vec!["acc-2".to_string()]);
    }

    #[test]
    fn requested_id_absent_errors() {
        let u = user();
        let result = scoped_account_ids(&u, Some("acc-3"));
        assert!(result.is_err());
    }

    #[test]
    fn empty_account_ids_with_no_request_returns_empty() {
        let u = CurrentUser {
            id: "default".to_string(),
            account_ids: vec![],
        };
        let result = scoped_account_ids(&u, None).expect("should succeed");
        assert!(result.is_empty());
    }
}
