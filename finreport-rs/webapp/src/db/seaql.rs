use migration::{Migrator, MigratorTrait};
use sea_orm::{Database, DatabaseConnection};
use tracing::info;
use utils::settings::Settings;

/// Connects to `database_url` and, unless `APP_run_migrations=false`, applies
/// pending migrations before returning the connection. Reads the flag itself
/// via a fresh `Settings::from_env()` (falling back to the enabled default on
/// any load error) so every existing caller (`webapp::main`, `user-admin`,
/// the auth integration tests) keeps compiling and behaving exactly as
/// before — unaware there is now a flag at all — while still honoring it.
///
/// Callers that already have a `Settings` in scope (`projector`) should use
/// [`init_db_with_migrations`] instead, to avoid the extra `Settings::from_env()`
/// call and to keep the decision visibly tied to the settings they already
/// loaded.
pub async fn init_db(database_url: &str) -> Result<DatabaseConnection, std::io::Error> {
    let run_migrations = Settings::from_env()
        .map(|settings| settings.run_migrations())
        .unwrap_or(true);
    init_db_with_migrations(database_url, run_migrations).await
}

/// Connects to `database_url` and applies pending migrations only if
/// `run_migrations` is `true`. `false` is the prod shape (deploy runbook):
/// migrations then run only via the `migration` binary's own explicit step,
/// never as a side effect of `webapp`/`projector` starting or restarting.
pub async fn init_db_with_migrations(
    database_url: &str,
    run_migrations: bool,
) -> Result<DatabaseConnection, std::io::Error> {
    let conn = Database::connect(database_url)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    if run_migrations {
        Migrator::up(&conn, None)
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    } else {
        info!(
            "[db] APP_run_migrations=false; skipping Migrator::up — apply migrations via the \
             deploy runbook's explicit `migration` binary step"
        );
    }

    Ok(conn)
}
