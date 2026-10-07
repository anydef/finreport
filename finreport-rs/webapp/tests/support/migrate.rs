//! Runs the real migration set against a connection — the same
//! `migration::Migrator` `webapp`'s startup and `make migrate` use, so the
//! integration suite exercises the actual schema (§3), not a hand-rolled
//! approximation of it.
//!
//! §9.2: migrations run once per test process. Every `#[tokio::test]` in
//! `tests/common/mod.rs` used to call `seaql::init_db` (which runs
//! `Migrator::up` unconditionally) against the **same** shared
//! `finreport-wp4-pg` container, so parallel tests in one binary — and
//! parallel tests across binaries that share the container — raced
//! `sea-orm-migration`'s own tracking table. [`run_migrations_once`]
//! memoizes the result per database URL, process-wide, so the second and
//! later callers for the same URL just await the first caller's outcome
//! instead of re-running `Migrator::up` concurrently.
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use migration::{DbErr, Migrator, MigratorTrait};
use sea_orm::DatabaseConnection;

/// Applies every migration in order. Idempotent: safe to call again on an
/// already-migrated connection (sea-orm-migration tracks what it has run).
/// Prefer [`run_migrations_once`] when several tests in the same process
/// might call this against the same database concurrently.
pub async fn run_migrations(connection: &DatabaseConnection) -> Result<(), DbErr> {
    Migrator::up(connection, None).await
}

/// One cell per distinct database URL, shared by every test in this process.
/// `tokio::sync::OnceCell` rather than `std::sync::OnceLock` because the
/// initializing future is itself async (`Migrator::up`).
type MigrationCell = tokio::sync::OnceCell<Result<(), String>>;

static MIGRATION_CELLS: LazyLock<Mutex<HashMap<String, &'static MigrationCell>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Runs `Migrator::up` against `database_url` at most once per process,
/// regardless of how many tests — in this binary, or another one linked
/// against the same `tests/support` copy — call it concurrently for the same
/// URL. Later callers for the same URL await the first call's result instead
/// of racing `sea-orm-migration`'s tracking table.
///
/// `DbErr` is not `Clone`, so the cell stores `Result<(), String>` and a
/// cached failure is re-surfaced as `DbErr::Migration` rather than retried —
/// retrying would either repeat the failure or (worse) interleave a second
/// `Migrator::up` with whatever the first call left half-applied.
pub async fn run_migrations_once(
    database_url: &str,
    connection: &DatabaseConnection,
) -> Result<(), DbErr> {
    let cell: &'static MigrationCell = {
        let mut cells = MIGRATION_CELLS
            .lock()
            .expect("migration cell registry mutex poisoned");
        *cells
            .entry(database_url.to_string())
            .or_insert_with(|| Box::leak(Box::new(MigrationCell::new())))
    };

    let result = cell
        .get_or_init(|| async { run_migrations(connection).await.map_err(|e| e.to_string()) })
        .await;

    result.clone().map_err(DbErr::Migration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use testcontainers::runners::AsyncRunner;
    use testcontainers::ImageExt;
    use testcontainers_modules::postgres::Postgres;
    use tokio::task::JoinSet;

    /// Several concurrent callers against the same freshly migrated
    /// container must all succeed and must not race `Migrator::up` against
    /// itself — the bug `run_migrations_once` exists to fix (§9.2).
    #[tokio::test]
    async fn concurrent_callers_for_the_same_url_all_succeed() {
        let container = Postgres::default()
            .with_db_name("finreport")
            .with_user("finreport")
            .with_password("finreport")
            .with_tag("17-alpine")
            .start()
            .await
            .expect("start Postgres testcontainer");
        let host = container.get_host().await.expect("testcontainer host");
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("testcontainer mapped port");
        let database_url = format!("postgres://finreport:finreport@{host}:{port}/finreport");

        let connection = Arc::new(
            sea_orm::Database::connect(&database_url)
                .await
                .expect("connect to testcontainer"),
        );

        let mut calls = JoinSet::new();
        for _ in 0..20 {
            let database_url = database_url.clone();
            let connection = connection.clone();
            calls.spawn(async move { run_migrations_once(&database_url, &connection).await });
        }

        while let Some(result) = calls.join_next().await {
            result
                .expect("task panicked")
                .expect("every concurrent caller should observe success");
        }
    }
}
