pub use sea_orm_migration::prelude::*;

mod m20220101_000001_account;
mod m20250609_193042_account_balances;
mod m20250609_221755_account_transactions;
mod m20260718_000001_idx_account_transactions_account_booking;
mod m20260820_000001_account_name;

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
        ]
    }
}
