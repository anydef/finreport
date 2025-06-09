use migration::{Migrator, MigratorTrait};
use sea_orm::{Database, DatabaseConnection};

pub async fn init_db(database_url: &str) -> Result<DatabaseConnection, std::io::Error> {
    let conn = Database::connect(database_url)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    Migrator::up(&conn, None).await;

    Ok(conn)
}
