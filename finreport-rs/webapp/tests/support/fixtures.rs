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

use std::fs;
use std::path::Path;
use std::time::Duration;

use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::ClientConfig;
use serde::Deserialize;
use serde_json::Value;

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
