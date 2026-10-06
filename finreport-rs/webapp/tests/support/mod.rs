//! Shared test-only infrastructure for the projector's integration tests
//! (§9's "done when" acceptance criteria): a throwaway Postgres container,
//! a throwaway Redpanda container, and a loader for the WP0 fixture corpus.
//!
//! WP1's `fixture-replay` binary isn't implemented on this branch yet, so
//! these tests publish the fixture corpus themselves via a small helper
//! built from `webapp/fixtures/manifest.json` directly.

#![allow(dead_code)] // Not every test file in this directory uses every helper.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use chrono::{DateTime, Utc};
use rdkafka::message::{Header, OwnedHeaders};
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use serde_json::Value;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use testcontainers_modules::postgres::Postgres;

use webapp::db::seaql;
use webapp::kafka::envelope::Envelope;
use webapp::projection::ConsumedRecord;

/// Container-name prefix every throwaway Postgres this test binary starts
/// uses -- distinct from every other agent's throwaway Postgres on this
/// shared machine. Each call to `start_postgres` suffixes it with a
/// process-unique sequence number: several tests reset state by starting a
/// *second* fresh database mid-test, and Docker won't let two containers
/// share a name even when the first's (asynchronous) teardown has only been
/// requested, not finished -- a shared fixed name would race. The host port
/// is likewise left to Docker to assign (`get_host_port_ipv4`) rather than
/// pinned, for the same reason.
const PG_CONTAINER_PREFIX: &str = "finreport-wp3-pg";
/// Container-name prefix every throwaway Redpanda this test binary starts
/// uses, for the same reason as `PG_CONTAINER_PREFIX`.
const REDPANDA_CONTAINER_PREFIX: &str = "finreport-wp3-rp";
/// Host port `finreport-wp3-rp` is pinned to (Redpanda's
/// `--advertise-kafka-addr` must name a port up front, unlike Postgres).
pub const REDPANDA_HOST_PORT: u16 = 19292;

static CONTAINER_SEQ: AtomicU32 = AtomicU32::new(0);

/// A process-unique suffix so repeated `start_postgres`/`start_redpanda`
/// calls never collide on container name.
fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        CONTAINER_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// A started, migrated Postgres container plus the connection pooled against
/// it. Dropping this drops the container (testcontainers' own cleanup).
pub struct TestDb {
    pub conn: DatabaseConnection,
    _container: ContainerAsync<Postgres>,
}

/// Starts a fresh `finreport-wp3-pg-*` container on a Docker-assigned host
/// port and runs every migration against it, exactly like
/// `webapp::db::seaql::init_db` does for the real server.
pub async fn start_postgres() -> TestDb {
    let name = format!("{PG_CONTAINER_PREFIX}-{}", unique_suffix());
    let container = Postgres::default()
        .with_db_name("finreport_wp3_test")
        .with_user("finreport")
        .with_password("finreport")
        .with_container_name(name)
        .start()
        .await
        .expect("failed to start finreport-wp3-pg");

    let port = container
        .get_host_port_ipv4(5432.tcp())
        .await
        .expect("failed to read finreport-wp3-pg's mapped port");
    let database_url = format!(
        "postgres://finreport:finreport@127.0.0.1:{port}/finreport_wp3_test"
    );
    let conn = seaql::init_db(&database_url)
        .await
        .expect("failed to connect/migrate finreport-wp3-pg");

    TestDb {
        conn,
        _container: container,
    }
}

/// A started Redpanda container, reachable at `127.0.0.1:{REDPANDA_HOST_PORT}`.
pub struct TestBroker {
    pub brokers: String,
    _container: ContainerAsync<GenericImage>,
}

/// Starts a fresh single-node `finreport-wp3-rp-*` Redpanda broker, on
/// `REDPANDA_HOST_PORT`. The host port is fixed up front (rather than
/// discovered after start) so `--advertise-kafka-addr` can name it directly,
/// which is what lets a client outside the container actually connect.
pub async fn start_redpanda() -> TestBroker {
    let advertise = format!("PLAINTEXT://127.0.0.1:{REDPANDA_HOST_PORT}");
    let listen = format!("PLAINTEXT://0.0.0.0:{REDPANDA_HOST_PORT}");

    let image = GenericImage::new("redpandadata/redpanda", "v24.2.18")
        .with_wait_for(WaitFor::message_on_stdout("Successfully started Redpanda!"))
        .with_cmd([
            "redpanda",
            "start",
            "--smp",
            "1",
            "--memory",
            "512M",
            "--overprovisioned",
            "--node-id",
            "0",
            "--check=false",
            "--kafka-addr",
            &listen,
            "--advertise-kafka-addr",
            &advertise,
        ])
        .with_container_name(format!("{REDPANDA_CONTAINER_PREFIX}-{}", unique_suffix()))
        .with_mapped_port(REDPANDA_HOST_PORT, REDPANDA_HOST_PORT.tcp());

    let container = image.start().await.expect("failed to start finreport-wp3-rp");

    TestBroker {
        brokers: format!("127.0.0.1:{REDPANDA_HOST_PORT}"),
        _container: container,
    }
}

// ---------------------------------------------------------------------------
// Fixture corpus loading (manifest.json -> ConsumedRecord), since WP1's
// fixture-replay binary isn't on this branch.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    topic: String,
    key: String,
    payload: String,
    #[serde(default)]
    headers: HashMap<String, Value>,
}

fn fixtures_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn header_string(headers: &HashMap<String, Value>, key: &str) -> Option<String> {
    headers.get(key).map(|v| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

/// Builds the `OwnedHeaders` a real Kafka record for this manifest entry
/// would carry — only the headers `manifest.json` actually lists, exactly
/// like a real headerless (or partially-headered) phase-1 record (§2.2).
fn manifest_headers(entry: &ManifestEntry) -> OwnedHeaders {
    const ORDERED_KEYS: &[&str] = &[
        "source",
        "source_account_id",
        "origin",
        "schema_version",
        "imported_at",
        "comdirect_account_key",
        "comdirect_account_name",
    ];

    let mut values: Vec<(&'static str, String)> = Vec::new();
    for key in ORDERED_KEYS {
        if let Some(value) = header_string(&entry.headers, key) {
            values.push((key, value));
        }
    }

    let mut headers = OwnedHeaders::new();
    for (key, value) in &values {
        headers = headers.insert(Header {
            key,
            value: Some(value.as_str()),
        });
    }
    headers
}

/// Loads the whole `webapp/fixtures/manifest.json` corpus as `ConsumedRecord`s
/// with sequential per-`(topic, partition 0)` offsets, exactly as the real
/// projector would see them after a from-scratch Redpanda replay.
pub fn load_fixture_records() -> Vec<ConsumedRecord> {
    let manifest_path = fixtures_root().join("manifest.json");
    let manifest_bytes = std::fs::read(&manifest_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", manifest_path.display()));
    let entries: Vec<ManifestEntry> = serde_json::from_slice(&manifest_bytes)
        .unwrap_or_else(|e| panic!("parsing {}: {e}", manifest_path.display()));

    let mut next_offset: HashMap<String, i64> = HashMap::new();
    let mut records = Vec::with_capacity(entries.len());

    for entry in &entries {
        let payload_path = fixtures_root().join(&entry.payload);
        let payload = std::fs::read(&payload_path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", payload_path.display()));

        let headers = manifest_headers(entry);
        let message_timestamp = header_string(&entry.headers, "imported_at")
            .and_then(|v| DateTime::parse_from_rfc3339(&v).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let envelope = Envelope::parse(Some(&headers), message_timestamp);

        let offset = next_offset.entry(entry.topic.clone()).or_insert(0);
        records.push(ConsumedRecord {
            topic: entry.topic.clone(),
            partition: 0,
            offset: *offset,
            key: Some(entry.key.clone()),
            payload,
            envelope,
        });
        *offset += 1;
    }

    records
}

/// `load_fixture_records`, but as the raw `(topic, key, payload bytes,
/// headers)` the real Redpanda-publishing end-to-end test needs instead of
/// the already-decoded `ConsumedRecord` the Postgres-only tests use.
pub fn load_fixture_publish_entries() -> Vec<(String, String, Vec<u8>, OwnedHeaders)> {
    let manifest_path = fixtures_root().join("manifest.json");
    let manifest_bytes = std::fs::read(&manifest_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", manifest_path.display()));
    let entries: Vec<ManifestEntry> = serde_json::from_slice(&manifest_bytes)
        .unwrap_or_else(|e| panic!("parsing {}: {e}", manifest_path.display()));

    entries
        .iter()
        .map(|entry| {
            let payload_path = fixtures_root().join(&entry.payload);
            let payload = std::fs::read(&payload_path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", payload_path.display()));
            (
                entry.topic.clone(),
                entry.key.clone(),
                payload,
                manifest_headers(entry),
            )
        })
        .collect()
}
