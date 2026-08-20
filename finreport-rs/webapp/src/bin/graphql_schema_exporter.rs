use sea_orm::{DatabaseBackend, MockDatabase};
use std::error::Error;
use std::path::Path;
use tokio::fs;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // No live DB is needed to export the schema SDL: a mock connection is
    // enough to satisfy create_schema's signature.
    let conn = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
    let schema = webapp::graphql::create_schema(conn);
    let sdl = schema.sdl();
    // Ensure the directory exists
    let dir_path = Path::new("graphql");
    if !dir_path.exists() {
        fs::create_dir_all(dir_path).await?;
    }
    // Write the SDL contents to graphql/schema.graphql
    fs::write("webapp/graphql/schema.graphql", sdl).await?;
    println!("GraphQL schema export written to graphql/schema.graphql");

    Ok(())
}
