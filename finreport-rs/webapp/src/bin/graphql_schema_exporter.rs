use sea_orm::{DatabaseBackend, MockDatabase};
use std::error::Error;
use tokio::fs;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // No live DB is needed to export the schema SDL: a mock connection is
    // enough to satisfy create_schema's signature.
    let conn = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
    let schema = webapp::graphql::create_schema(conn);
    let sdl = schema.sdl();
    // Run from `finreport-rs/` (as `just`/CI do); this is the canonical,
    // committed SDL per §5 — mirrored (not symlinked) into
    // `finreport-fe/src/lib/graphql/schema.graphql`.
    fs::write("webapp/schema.graphql", sdl).await?;
    println!("GraphQL schema export written to webapp/schema.graphql");

    Ok(())
}
