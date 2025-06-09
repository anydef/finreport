pub use sea_orm_migration::prelude::*;

mod m20220101_000001_account;
mod m20250609_193042_account_balances;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20220101_000001_account::Migration),
            Box::new(m20250609_193042_account_balances::Migration),
        ]
    }
}
