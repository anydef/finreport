//! `accounts` (§5): scoped account listing. `latestBalance` is prefetched
//! for the whole batch in one query (`loaders::latest_balances`) rather than
//! once per account.

use entity::entities::account;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::graphql::loaders;
use crate::graphql::scalars::Uuid as GqlUuid;
use crate::graphql::types::Account;

pub async fn fetch_accounts(
    db: &DatabaseConnection,
    user_id: Uuid,
    scoped_ids: &[Uuid],
) -> async_graphql::Result<Vec<Account>> {
    if scoped_ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows = account::Entity::find()
        .filter(account::Column::Id.is_in(scoped_ids.to_vec()))
        .all(db)
        .await?;

    let mut balances = loaders::latest_balances(db, scoped_ids).await?;
    let savings = crate::graphql::savings::flagged_accounts(db, user_id, scoped_ids).await?;

    let accounts = rows
        .into_iter()
        .map(|row| Account {
            latest_balance: balances.remove(&row.id),
            is_savings: savings.contains(&row.id),
            id: GqlUuid(row.id),
            source: row.source,
            external_id: row.external_id,
            display_id: row.display_id,
            account_type: row.account_type,
            iban: row.iban,
            bic: row.bic,
            institute: row.institute,
            label: row.label,
            currency: row.currency,
        })
        .collect();
    Ok(accounts)
}
