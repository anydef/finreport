pub use sea_orm_migration::prelude::*;

mod m20220101_000001_account;
mod m20250609_193042_account_balances;
mod m20250609_221755_account_transactions;
mod m20260718_000001_idx_account_transactions_account_booking;
mod m20260820_000001_account_name;
pub mod m20261006_000001_source_agnostic_read_model;
mod m20261006_000002_users;
mod m20261101_000001_categories;
mod m20261101_000002_labels;
mod m20261101_000003_rules;
mod m20261101_000004_counterparty_key;
mod m20261101_000005_app_user_is_admin;
mod m20261201_000001_tags;
mod m20261201_000002_insights;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20220101_000001_account::Migration),
            Box::new(m20250609_193042_account_balances::Migration),
            Box::new(m20250609_221755_account_transactions::Migration),
            Box::new(m20260718_000001_idx_account_transactions_account_booking::Migration),
            Box::new(m20260820_000001_account_name::Migration),
            // §2.8: renames the three Comdirect-specific tables to `legacy_*`
            // and creates the new source-agnostic read model beside them.
            Box::new(m20261006_000001_source_agnostic_read_model::Migration),
            // Depends on the new `account` table above (user_account.account_id).
            Box::new(m20261006_000002_users::Migration),
            // Iteration 2 §3: categories, labels, rules + the derived
            // counterparty_key column. Registered in dependency order even
            // though none of these carry foreign keys (§3 "No foreign keys
            // on these five tables").
            Box::new(m20261101_000001_categories::Migration),
            Box::new(m20261101_000002_labels::Migration),
            Box::new(m20261101_000003_rules::Migration),
            Box::new(m20261101_000004_counterparty_key::Migration),
            // Admin bootstrap (webapp::auth::bootstrap): marks the
            // bootstrap-managed admin user so it's distinguishable from
            // ordinary `user-admin create-user` accounts.
            Box::new(m20261101_000005_app_user_is_admin::Migration),
            // Iteration 3 §2.3: tags + the recurring override column, then
            // the detector's insights projection (independent of tags, but
            // registered after so a fresh DB always has the override column
            // before anything reads `transaction_user_label.recurring`).
            Box::new(m20261201_000001_tags::Migration),
            Box::new(m20261201_000002_insights::Migration),
        ]
    }
}
