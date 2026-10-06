use sea_orm::{DatabaseBackend, MockDatabase};
use secrecy::SecretString;
use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use tokio::fs;
use utils::settings::Settings;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // No live DB is needed to export the schema SDL: a mock connection is
    // enough to satisfy create_schema's signature.
    let conn = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
    // Likewise, the exported SDL doesn't depend on any setting's value —
    // only `create_schema`'s signature needs one.
    let settings = Arc::new(Settings {
        oauth_url: String::new(),
        url: String::new(),
        save_file_path: String::new(),
        database_url: SecretString::from(String::new()),
        kafka_brokers: None,
        cookie_secure: true,
        allowed_origins: String::new(),
        session_ttl_days: 30,
        projector_default_owner: None,
        accounts: BTreeMap::new(),
        account_name: None,
        client_id: None,
        client_secret: None,
        zugangsnummer: None,
        pin: None,
    });
    let schema = webapp::graphql::create_schema(Arc::new(conn), settings);
    let sdl = schema.sdl();
    // Run from `finreport-rs/` (as `just`/CI do); this is the canonical,
    // committed SDL per §5 — mirrored (not symlinked) into
    // `finreport-fe/src/lib/graphql/schema.graphql`.
    fs::write("webapp/schema.graphql", sdl).await?;
    println!("GraphQL schema export written to webapp/schema.graphql");

    Ok(())
}
