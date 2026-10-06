//! Runs the real migration set against a connection — the same
//! `migration::Migrator` `webapp`'s startup and `make migrate` use, so the
//! integration suite exercises the actual schema (§3), not a hand-rolled
//! approximation of it.

use migration::{DbErr, Migrator, MigratorTrait};
use sea_orm::DatabaseConnection;

/// Applies every migration in order. Idempotent: safe to call again on an
/// already-migrated connection (sea-orm-migration tracks what it has run).
pub async fn run_migrations(connection: &DatabaseConnection) -> Result<(), DbErr> {
    Migrator::up(connection, None).await
}
