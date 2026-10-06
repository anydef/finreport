//! Smoke test for the shared testcontainers harness itself (§9 WP6 "done
//! when" — the harness must be exercised, not just assumed to work). WP3 and
//! WP4's own integration tests (projector replay, GraphQL-over-seeded-data)
//! are their files, not this one; this one only proves `tests/support/`
//! starts Postgres + Kafka, migrates, and publishes the fixture corpus
//! correctly enough for those to build on.
//!
//! Gated behind the `integration` feature (§9 shared-file protocol): `just
//! test` never compiles this file; `just test-integration` does.

#![cfg(feature = "integration")]

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::{ClientConfig, Message, TopicPartitionList};
use sea_orm::{ConnectionTrait, Statement};

use support::{publish_fixture_corpus, TestKafka, TestPostgres};
use webapp::kafka::envelope::{self, TOPIC_TRANSACTION};

fn fixtures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Postgres half of the harness: a fresh container ends up with every
/// migration applied, including the §2.8 reshape (so both the renamed
/// `legacy_*` tables and the new source-agnostic read model exist side by
/// side, and the old `account` name — now `legacy_account` — is gone).
#[tokio::test]
async fn postgres_harness_applies_every_migration() {
    let pg = TestPostgres::start().await;

    let rows = pg
        .connection()
        .query_all(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'",
        ))
        .await
        .expect("list tables");
    let tables: Vec<String> = rows
        .iter()
        .map(|row| row.try_get::<String>("", "table_name").unwrap())
        .collect();

    for expected in [
        "account",
        "account_balance",
        "transaction",
        "projection_offset",
        "legacy_account",
        "legacy_account_balance",
        "legacy_account_transactions",
        "app_user",
        "user_account",
        "user_session",
    ] {
        assert!(
            tables.iter().any(|t| t == expected),
            "expected table {expected:?} after migrating, got {tables:?}"
        );
    }
}

/// Kafka half of the harness: the four finreport topics exist on a fresh
/// container and the fixture corpus publishes onto them with the keys and
/// headers `manifest.json` lists — including the headerless phase-1-style
/// record and the `legacy-backfill` record, both edge cases §2.2/§2.8 exist
/// to exercise.
#[tokio::test]
async fn fixture_corpus_publishes_with_expected_envelopes() {
    let kafka = TestKafka::start().await;
    let records = publish_fixture_corpus(kafka.bootstrap_servers(), &fixtures_dir()).await;
    assert!(!records.is_empty(), "fixture manifest should not be empty");

    // Every (topic, key) the manifest promised must show up on the broker,
    // with its headers parsed back through the same `Envelope::parse` the
    // projector will use — proving the corpus and the shared contract agree,
    // not just that bytes made it onto a topic.
    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", kafka.bootstrap_servers())
        .set("group.id", "harness-smoke-test")
        .set("enable.auto.commit", "false")
        .set("auto.offset.reset", "earliest")
        .create()
        .expect("create consumer");

    let mut assignment = TopicPartitionList::new();
    for topic in records
        .iter()
        .map(|r| r.topic.as_str())
        .collect::<std::collections::HashSet<_>>()
    {
        assignment.add_partition(topic, 0);
    }
    // Explicit assign (no subscribe, no consumer group coordination) means
    // partitions start at `Offset::Invalid` unless told otherwise — set every
    // one to the beginning so this reads back everything just published,
    // mirroring how the projector itself starts from `Offset::Beginning`
    // when it has no stored `projection_offset` row (§2.3).
    assignment
        .set_all_offsets(rdkafka::Offset::Beginning)
        .expect("seek every assigned partition to the beginning");
    consumer.assign(&assignment).expect("assign partitions");

    // The corpus deliberately has a few repeated (topic, key) pairs — e.g.
    // multiple `finreport.account-balance` snapshots for the same account,
    // to exercise log-compaction/"latest wins" semantics — so track total
    // messages consumed separately from the last-envelope-per-key map used
    // for the edge-case assertions below.
    let mut seen: HashMap<(String, String), envelope::Envelope> = HashMap::new();
    let mut consumed = 0usize;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while consumed < records.len() && std::time::Instant::now() < deadline {
        match consumer.poll(Duration::from_millis(200)) {
            // Transient consumer-group-coordinator errors can surface for a
            // moment right after the broker/topics come up even though we
            // never subscribe or commit — just keep polling until the
            // deadline instead of failing on the first blip.
            Some(Err(e)) => eprintln!("transient consumer error, retrying: {e}"),
            None => {}
            Some(Ok(message)) => {
                let key =
                    String::from_utf8_lossy(message.key().expect("message key")).into_owned();
                let timestamp = message
                    .timestamp()
                    .to_millis()
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .unwrap_or_else(Utc::now);
                let parsed = envelope::Envelope::parse(message.headers(), timestamp);
                seen.insert((message.topic().to_string(), key), parsed);
                consumed += 1;
            }
        }
    }

    assert_eq!(
        consumed,
        records.len(),
        "expected to consume every published fixture record back"
    );

    // Headerless phase-1-style record: no headers on the wire, but
    // `Envelope::parse` fills in the §2.2 defaults rather than erroring.
    let phase1 = seen
        .get(&(TOPIC_TRANSACTION.to_string(), "ACC1-PHASE1-0001".to_string()))
        .expect("phase-1-style fixture record");
    assert_eq!(phase1.source, "comdirect");
    assert_eq!(phase1.origin, "source");
    assert_eq!(phase1.schema_version, 1);
    assert!(envelope::is_bank_verbatim(phase1));

    // Legacy-backfill reconstruction: `origin=legacy-backfill` must round-trip
    // as non-bank-verbatim — the one documented exception to "value is always
    // raw bank bytes" (§2.8).
    let legacy = seen
        .get(&(TOPIC_TRANSACTION.to_string(), "LEGACY-0001".to_string()))
        .expect("legacy-backfill fixture record");
    assert_eq!(legacy.origin, "legacy-backfill");
    assert!(!envelope::is_bank_verbatim(legacy));

    // Missing source_account_id: present in the manifest, but the header the
    // projector cannot do without — harness proves the corpus actually omits
    // it rather than asserting projector behavior (that's WP3's test).
    let missing_account = seen
        .get(&(
            TOPIC_TRANSACTION.to_string(),
            "ACC1-MISSING-ACCOUNT-0001".to_string(),
        ))
        .expect("missing-account fixture record");
    assert!(missing_account.source_account_id.is_none());
}
