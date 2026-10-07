//! Publishes the WP0 fixture corpus onto the ingest topics for local/CI
//! replay against the projector (§7/§8).
//!
//! Reads `<fixtures-dir>/manifest.json` — the ordered list of records WP0
//! curated, each naming a topic, key, payload file and the exact headers that
//! record carries (`fixtures/README.md`) — and publishes every one verbatim.
//! Unlike the importer, this binary does not build headers from a uniform
//! `RecordMeta`: several manifest records are deliberately headerless or
//! partially headered (phase-1-style, missing `source_account_id`, …) to
//! exercise `envelope::Envelope::parse`'s defaults downstream, so the published
//! headers are exactly whatever the manifest lists for that record, nothing
//! added or inferred.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rdkafka::message::{Header, OwnedHeaders};
use serde::Deserialize;
use serde_json::{Map, Value};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use webapp::kafka::producer::EventPublisher;

/// One row of `manifest.json`.
#[derive(Deserialize)]
struct ManifestRecord {
    topic: String,
    key: String,
    /// Path to the payload file, relative to the fixtures directory.
    payload: String,
    /// Only the headers this record actually carries — a key absent here is
    /// a header never set on the wire, exactly like a real phase-1 record.
    #[serde(default)]
    headers: Map<String, Value>,
    #[serde(default)]
    note: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let fixtures_dir = fixtures_dir_arg(std::env::args().skip(1))
        .map_err(|e| format!("{e}; usage: fixture-replay <fixtures-dir>"))?;
    let brokers = std::env::var("APP_kafka_brokers")
        .map_err(|_| "APP_kafka_brokers is required to publish the fixture corpus")?;

    let publisher = EventPublisher::connect(&brokers)?;
    let manifest = load_manifest(&fixtures_dir)?;

    info!(
        fixtures_dir = %fixtures_dir.display(),
        records = manifest.len(),
        "[fixture-replay] publishing fixture corpus"
    );

    let mut published_per_topic: HashMap<String, u32> = HashMap::new();
    let mut failures = 0u32;
    for record in &manifest {
        let payload_path = fixtures_dir.join(&record.payload);
        let payload = fs::read(&payload_path)
            .map_err(|e| format!("reading {}: {e}", payload_path.display()))?;
        let headers = build_headers(&record.headers);

        match publisher
            .publish_with_headers(&record.topic, &record.key, &payload, headers)
            .await
        {
            Ok(()) => {
                *published_per_topic.entry(record.topic.clone()).or_insert(0) += 1;
                info!(
                    topic = %record.topic,
                    key = %record.key,
                    note = record.note.as_deref().unwrap_or(""),
                    "published fixture record"
                );
            }
            Err(e) => {
                failures += 1;
                error!(topic = %record.topic, key = %record.key, %e, "failed to publish fixture record");
            }
        }
    }

    for (topic, count) in &published_per_topic {
        info!(%topic, count, "[fixture-replay] topic filled");
    }

    if failures > 0 {
        return Err(format!("{failures} fixture record(s) failed to publish").into());
    }

    Ok(())
}

/// Reads and parses `<dir>/manifest.json`.
fn load_manifest(dir: &Path) -> Result<Vec<ManifestRecord>, Box<dyn std::error::Error>> {
    let manifest_path = dir.join("manifest.json");
    let raw = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("reading {}: {e}", manifest_path.display()))?;
    let records: Vec<ManifestRecord> = serde_json::from_str(&raw)
        .map_err(|e| format!("parsing {}: {e}", manifest_path.display()))?;
    Ok(records)
}

/// Builds Kafka headers from the manifest's per-record `headers` object,
/// setting exactly the keys present — no defaulting, no inference. That is
/// what lets the manifest describe headerless and partially-headered records
/// (§2.2 phase-1 compatibility, the missing-`source_account_id` skip path).
fn build_headers(fields: &Map<String, Value>) -> OwnedHeaders {
    let mut headers = OwnedHeaders::new();
    for (key, value) in fields {
        let rendered = header_value(value);
        headers = headers.insert(Header {
            key: key.as_str(),
            value: Some(rendered.as_str()),
        });
    }
    headers
}

/// Renders a manifest header value as the string it is published as.
/// `schema_version` is authored as a JSON number in the manifest but travels
/// on the wire as text, like every other header.
fn header_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Reads the single positional `<fixtures-dir>` argument.
fn fixtures_dir_arg(args: impl Iterator<Item = String>) -> Result<PathBuf, String> {
    let args: Vec<String> = args.collect();
    match args.as_slice() {
        [dir] => Ok(PathBuf::from(dir)),
        [] => Err("missing <fixtures-dir> argument".to_string()),
        _ => Err(format!(
            "expected exactly one argument, got {}: {args:?}",
            args.len()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webapp::kafka::envelope::{Envelope, ORIGIN_LEGACY_BACKFILL, SOURCE_COMDIRECT};

    fn fields(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn build_headers_sets_only_the_fields_present() {
        let fields = fields(&[
            ("comdirect_account_key", Value::String("0".to_string())),
            ("imported_at", Value::String("2024-05-01T12:00:00Z".to_string())),
        ]);

        let headers = build_headers(&fields);
        let envelope = Envelope::parse(Some(&headers), chrono::Utc::now());

        // Absent fields fall back to the §2.2 phase-1 defaults, exactly like
        // a real headerless record — this is not a shortcut the test takes,
        // it is the behavior the manifest's headerless rows rely on.
        assert_eq!(envelope.source, SOURCE_COMDIRECT);
        assert_eq!(envelope.source_account_id, None);
        assert_eq!(envelope.comdirect_account_key.as_deref(), Some("0"));
    }

    #[test]
    fn build_headers_renders_numeric_schema_version_as_text() {
        let fields = fields(&[
            ("schema_version", Value::Number(1.into())),
            ("source", Value::String(SOURCE_COMDIRECT.to_string())),
        ]);

        let headers = build_headers(&fields);
        let envelope = Envelope::parse(Some(&headers), chrono::Utc::now());

        assert_eq!(envelope.schema_version, 1);
    }

    #[test]
    fn build_headers_round_trips_legacy_backfill_origin() {
        let fields = fields(&[
            ("origin", Value::String(ORIGIN_LEGACY_BACKFILL.to_string())),
            ("source_account_id", Value::String("ACC-1".to_string())),
        ]);

        let headers = build_headers(&fields);
        let envelope = Envelope::parse(Some(&headers), chrono::Utc::now());

        assert_eq!(envelope.origin, ORIGIN_LEGACY_BACKFILL);
        assert_eq!(envelope.source_account_id.as_deref(), Some("ACC-1"));
    }

    #[test]
    fn fixtures_dir_arg_requires_exactly_one_argument() {
        assert!(fixtures_dir_arg(std::iter::empty()).is_err());
        assert!(fixtures_dir_arg(["a".to_string(), "b".to_string()].into_iter()).is_err());

        let dir = fixtures_dir_arg(["fixtures".to_string()].into_iter()).unwrap();
        assert_eq!(dir, PathBuf::from("fixtures"));
    }

    #[test]
    fn manifest_loads_and_resolves_payload_paths_relative_to_its_directory() {
        let manifest = load_manifest(Path::new("fixtures")).expect("fixture manifest exists");
        assert!(!manifest.is_empty());
        for record in &manifest {
            let payload_path = Path::new("fixtures").join(&record.payload);
            assert!(
                payload_path.exists(),
                "payload file referenced by manifest is missing: {}",
                payload_path.display()
            );
        }
    }
}
