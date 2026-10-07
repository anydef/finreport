//! Postgres container + migrated connection for the integration suite.

use sea_orm::{Database, DatabaseConnection};
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;

use super::migrate::run_migrations_once;

/// A running, already-migrated Postgres instance. Keep the container alive
/// for as long as the test needs the database — dropping it tears the
/// container down.
pub struct TestPostgres {
    _container: ContainerAsync<Postgres>,
    database_url: String,
    connection: DatabaseConnection,
}

impl TestPostgres {
    /// Starts a fresh Postgres container, applies every migration in
    /// `migration::Migrator` (the same ones `make migrate` / `webapp`'s
    /// startup run), and hands back a ready-to-query connection.
    ///
    /// Pinned to the 17-alpine tag deployed/used by `docker-compose.local.yml`,
    /// not the module's own (older) default, so integration tests run against
    /// the same major version as everywhere else.
    pub async fn start() -> Self {
        use testcontainers::ImageExt;

        let container = Postgres::default()
            .with_db_name("finreport")
            .with_user("finreport")
            .with_password("finreport")
            .with_tag("17-alpine")
            .start()
            .await
            .expect("start Postgres testcontainer");

        let host = container
            .get_host()
            .await
            .expect("Postgres testcontainer host");
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("Postgres testcontainer mapped port");
        let database_url = format!("postgresql://finreport:finreport@{host}:{port}/finreport");

        let connection = Database::connect(&database_url)
            .await
            .expect("connect to Postgres testcontainer");
        run_migrations_once(&database_url, &connection)
            .await
            .expect("run migrations against Postgres testcontainer");

        Self {
            _container: container,
            database_url,
            connection,
        }
    }

    /// Connection string, e.g. for a binary under test that takes
    /// `APP_database_url` directly instead of a `DatabaseConnection`.
    ///
    /// `harness_smoke.rs` only needs `connection()`; kept for other WPs'
    /// tests (e.g. spawning a bin under test with `APP_database_url` set).
    #[allow(dead_code)]
    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    /// A `sea-orm` connection into the migrated database.
    pub fn connection(&self) -> &DatabaseConnection {
        &self.connection
    }
}
