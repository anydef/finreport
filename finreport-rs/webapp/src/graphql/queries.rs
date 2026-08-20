use async_graphql::{Context, Object, Result as GqlResult, SimpleObject};
use chrono::NaiveDate;
use entity::entities::prelude::{
    Account as AccountEntity, AccountTransactions as AccountTransactionsEntity,
};
use entity::entities::{account, account_transactions};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use crate::graphql::current_user::{current_user, scoped_account_ids};

pub struct QueryRoot;

#[derive(SimpleObject)]
pub struct Account {
    pub id: i32,
    pub account_id: String,
    pub display_id: String,
    pub account_type: String,
    pub iban: String,
    pub bic: String,
    pub institute: String,
    /// Human-readable label of the Comdirect login this account was imported
    /// through. Null when that login has no label configured; use `accountId`
    /// to reference an account.
    pub account_name: Option<String>,
}

impl From<account::Model> for Account {
    fn from(m: account::Model) -> Self {
        Self {
            id: m.id,
            account_id: m.account_id,
            display_id: m.display_id,
            account_type: m.account_type,
            iban: m.iban,
            bic: m.bic,
            institute: m.institute,
            account_name: m.account_name,
        }
    }
}

#[derive(SimpleObject)]
pub struct Transaction {
    pub reference: String,
    pub account_id: String,
    pub booking_status: String,
    pub booking_date: String,
    pub amount: f64,
    pub remitter: String,
    pub deptor: String,
    pub creditor: String,
    pub creditor_id: String,
    pub creditor_mandate_id: String,
    pub remittance_info: String,
    pub transaction_type: String,
}

impl From<account_transactions::Model> for Transaction {
    fn from(m: account_transactions::Model) -> Self {
        Self {
            reference: m.reference,
            account_id: m.account_id,
            booking_status: m.booking_status,
            // NaiveDate's Display/to_string() is already ISO-8601 (YYYY-MM-DD).
            booking_date: m.booking_date.to_string(),
            amount: m.amount,
            remitter: m.remitter,
            deptor: m.deptor,
            creditor: m.creditor,
            creditor_id: m.creditor_id,
            creditor_mandate_id: m.creditor_mandate_id,
            remittance_info: m.remittance_info,
            transaction_type: m.transaction_type,
        }
    }
}

/// Parses an optional `"YYYY-MM-DD"` date string. `None` passes through as
/// `None`; an unparseable string yields a clear GraphQL error.
fn parse_date_opt(s: Option<&str>) -> GqlResult<Option<NaiveDate>> {
    match s {
        None => Ok(None),
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(Some)
            .map_err(|e| {
                async_graphql::Error::new(format!(
                    "invalid date '{s}', expected format YYYY-MM-DD: {e}"
                ))
            }),
    }
}

#[Object]
impl QueryRoot {
    async fn accounts(&self, ctx: &Context<'_>) -> GqlResult<Vec<Account>> {
        let db = ctx.data::<DatabaseConnection>()?;
        let user = current_user(db)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        let rows = AccountEntity::find()
            .filter(account::Column::AccountId.is_in(user.account_ids.clone()))
            .all(db)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        Ok(rows.into_iter().map(Account::from).collect())
    }

    async fn transactions(
        &self,
        ctx: &Context<'_>,
        start_date: Option<String>,
        end_date: Option<String>,
        account_id: Option<String>,
    ) -> GqlResult<Vec<Transaction>> {
        let db = ctx.data::<DatabaseConnection>()?;
        let user = current_user(db)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        let scoped_ids = scoped_account_ids(&user, account_id.as_deref())?;
        if scoped_ids.is_empty() {
            return Ok(vec![]);
        }

        let start = parse_date_opt(start_date.as_deref())?;
        let end = parse_date_opt(end_date.as_deref())?;

        let mut query = AccountTransactionsEntity::find()
            .filter(account_transactions::Column::AccountId.is_in(scoped_ids));
        if let Some(start) = start {
            query = query.filter(account_transactions::Column::BookingDate.gte(start));
        }
        if let Some(end) = end {
            query = query.filter(account_transactions::Column::BookingDate.lte(end));
        }

        let rows = query
            .order_by_desc(account_transactions::Column::BookingDate)
            .all(db)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        Ok(rows.into_iter().map(Transaction::from).collect())
    }

    async fn last_transaction_date(
        &self,
        ctx: &Context<'_>,
        account_id: Option<String>,
    ) -> GqlResult<Option<String>> {
        let db = ctx.data::<DatabaseConnection>()?;
        let user = current_user(db)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        let scoped_ids = scoped_account_ids(&user, account_id.as_deref())?;
        if scoped_ids.is_empty() {
            return Ok(None);
        }

        let row = AccountTransactionsEntity::find()
            .filter(account_transactions::Column::AccountId.is_in(scoped_ids))
            .order_by_desc(account_transactions::Column::BookingDate)
            .one(db)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        Ok(row.map(|m| m.booking_date.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_date_opt_none_passes_through() {
        assert_eq!(parse_date_opt(None).unwrap(), None);
    }

    #[test]
    fn parse_date_opt_valid_date_parses() {
        let parsed = parse_date_opt(Some("2026-01-15")).unwrap();
        assert_eq!(parsed, Some(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap()));
    }

    #[test]
    fn parse_date_opt_invalid_date_errors() {
        assert!(parse_date_opt(Some("not-a-date")).is_err());
    }
}
