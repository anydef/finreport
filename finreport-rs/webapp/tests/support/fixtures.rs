//! Publishes the WP0 fixture corpus (§7, `webapp/fixtures/manifest.json`)
//! onto a Kafka broker, exactly as `fixture-replay` does — so a projector
//! integration test (WP3) and a GraphQL-over-seeded-data test (WP4) seed
//! themselves from the one corpus everyone else already derives mocks and
//! expectations from, rather than inventing their own records.
//!
//! This duplicates the manifest-walking logic `fixture-replay` (WP1,
//! `src/bin/fixture_replay.rs`) owns for publishing against the interactive
//! dev stack — deliberately: the harness must not depend on a WP1 binary to
//! build, and a thin, read-only "replay this JSON" walk is cheap to keep in
//! step if the manifest shape changes.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::ClientConfig;
use serde::Deserialize;
use serde_json::Value;

use webapp::kafka::envelope::Envelope;
use webapp::projection::ConsumedRecord;

const PUBLISH_TIMEOUT: Duration = Duration::from_secs(10);

/// One record from `manifest.json`: which topic/key it publishes under, the
/// headers it carries (possibly none — the headerless phase-1 fixture), and
/// where its raw payload bytes live on disk.
#[derive(Debug)]
pub struct FixtureRecord {
    pub topic: String,
    pub key: String,
    pub payload: String,
    pub headers: Vec<(String, Value)>,
    #[allow(dead_code)]
    pub note: String,
}

// `manifest.json`'s `headers` is a JSON object, not the `Vec<(String, Value)>`
// above — a custom deserializer keeps call sites (`for (name, value) in
// &record.headers`) simple instead of matching on `serde_json::Map` everywhere.
impl<'de> serde::de::Deserialize<'de> for ManifestEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            topic: String,
            key: String,
            payload: String,
            #[serde(default)]
            headers: serde_json::Map<String, Value>,
            #[serde(default)]
            note: String,
        }

        let raw = Raw::deserialize(deserializer)?;
        Ok(ManifestEntry(FixtureRecord {
            topic: raw.topic,
            key: raw.key,
            payload: raw.payload,
            headers: raw.headers.into_iter().collect(),
            note: raw.note,
        }))
    }
}

struct ManifestEntry(FixtureRecord);

/// Reads `<fixtures_dir>/manifest.json` and publishes every record onto
/// `bootstrap_servers`, in manifest order, verbatim bytes with the headers
/// listed (a record absent from the manifest's `headers` carries none — the
/// headerless phase-1 fixture, §2.2). Blocks until every publish is
/// acknowledged.
pub async fn publish_fixture_corpus(bootstrap_servers: &str, fixtures_dir: &Path) -> Vec<FixtureRecord> {
    let manifest_path = fixtures_dir.join("manifest.json");
    let manifest_bytes = fs::read(&manifest_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display()));
    let entries: Vec<ManifestEntry> =
        serde_json::from_slice(&manifest_bytes).expect("parse fixtures manifest.json");

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", bootstrap_servers)
        .create()
        .expect("create fixture producer");

    let mut records = Vec::with_capacity(entries.len());
    for ManifestEntry(record) in entries {
        let payload_path = fixtures_dir.join(&record.payload);
        let payload = fs::read(&payload_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", payload_path.display()));

        let mut headers = OwnedHeaders::new();
        for (name, value) in &record.headers {
            let value = header_value_string(value);
            headers = headers.insert(Header {
                key: name.as_str(),
                value: Some(value.as_str()),
            });
        }

        producer
            .send(
                FutureRecord::to(&record.topic)
                    .key(&record.key)
                    .payload(&payload)
                    .headers(headers),
                PUBLISH_TIMEOUT,
            )
            .await
            .unwrap_or_else(|(e, _)| panic!("publish fixture {}: {e}", record.key));

        records.push(record);
    }

    records
}

/// Kafka headers are byte strings; the manifest's JSON values need stringifying
/// without the quotes `serde_json::Value::to_string()` would add around a
/// plain string (`schema_version: 1` must become `"1"`, not `"1"` wrapped
/// again in literal quote characters).
fn header_value_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Manifest -> in-process record loading (WP3's projector tests): no broker
// involved, just `manifest.json` decoded straight into the shapes the
// projector itself consumes. Kept alongside `publish_fixture_corpus` above
// rather than hand-rolling a second manifest reader, even though the header
// representation differs (`manifest_headers` below needs real
// `rdkafka::message::OwnedHeaders` to drive `Envelope::parse`, whereas
// `FixtureRecord::headers` is a plain `Vec<(String, Value)>`).
// ---------------------------------------------------------------------------

/// One `manifest.json` entry, decoded just far enough to build either a
/// `ConsumedRecord` (`process_batch`-driven tests) or a raw publish tuple
/// (`run`-over-a-real-broker tests) — see `load_fixture_records` and
/// `load_fixture_publish_entries` below.
#[derive(Debug, Deserialize)]
struct ProjectorManifestEntry {
    topic: String,
    key: String,
    payload: String,
    #[serde(default)]
    headers: HashMap<String, Value>,
}

fn fixtures_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn read_projector_manifest() -> Vec<ProjectorManifestEntry> {
    let manifest_path = fixtures_root().join("manifest.json");
    let manifest_bytes = fs::read(&manifest_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", manifest_path.display()));
    serde_json::from_slice(&manifest_bytes)
        .unwrap_or_else(|e| panic!("parsing {}: {e}", manifest_path.display()))
}

fn projector_header_string(headers: &HashMap<String, Value>, key: &str) -> Option<String> {
    headers.get(key).map(|v| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

/// Builds the `OwnedHeaders` a real Kafka record for this manifest entry
/// would carry — only the headers `manifest.json` actually lists, exactly
/// like a real headerless (or partially-headered) phase-1 record (§2.2).
fn projector_manifest_headers(entry: &ProjectorManifestEntry) -> OwnedHeaders {
    const ORDERED_KEYS: &[&str] = &[
        "source",
        "source_account_id",
        "origin",
        "schema_version",
        "imported_at",
        "comdirect_account_key",
        "comdirect_account_name",
    ];

    let mut headers = OwnedHeaders::new();
    for key in ORDERED_KEYS {
        if let Some(value) = projector_header_string(&entry.headers, key) {
            headers = headers.insert(Header {
                key,
                value: Some(value.as_str()),
            });
        }
    }
    headers
}

/// Loads the whole `webapp/fixtures/manifest.json` corpus as `ConsumedRecord`s
/// with sequential per-`(topic, partition 0)` offsets, exactly as the real
/// projector would see them after a from-scratch Redpanda/Kafka replay —
/// for tests that drive `projection::process_batch` directly, no broker
/// involved.
pub fn load_fixture_records() -> Vec<ConsumedRecord> {
    let entries = read_projector_manifest();
    let mut next_offset: HashMap<String, i64> = HashMap::new();
    let mut records = Vec::with_capacity(entries.len());

    for entry in &entries {
        let payload_path = fixtures_root().join(&entry.payload);
        let payload = fs::read(&payload_path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", payload_path.display()));

        let headers = projector_manifest_headers(entry);
        let message_timestamp = projector_header_string(&entry.headers, "imported_at")
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
/// headers)` tuples an end-to-end test that publishes to a real broker and
/// then drives `projection::run` needs instead of the already-decoded
/// `ConsumedRecord`s `process_batch`-driven tests use.
pub fn load_fixture_publish_entries() -> Vec<(String, String, Vec<u8>, OwnedHeaders)> {
    read_projector_manifest()
        .iter()
        .map(|entry| {
            let payload_path = fixtures_root().join(&entry.payload);
            let payload = fs::read(&payload_path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", payload_path.display()));
            (
                entry.topic.clone(),
                entry.key.clone(),
                payload,
                projector_manifest_headers(entry),
            )
        })
        .collect()
}
