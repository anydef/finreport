//! End-to-end projector test (§9 "done when"): publishes the whole WP0
//! fixture corpus to a throwaway Kafka broker, then runs the real
//! `projection::run` (with `--until-caught-up`) against a throwaway Postgres
//! and asserts on the resulting read model — the only test in this crate
//! that exercises the Kafka-facing half (`run`, `ConsumedRecord::from_message`,
//! offset `assign()`/watermark logic) rather than `process_batch` directly.
//! Uses WP6's shared `support::TestKafka`/`support::TestPostgres` harness
//! (a real Kafka broker via `testcontainers-modules`, not a hand-rolled
//! Redpanda `GenericImage` — same wire protocol, one fewer harness to keep
//! in step) rather than this test's original bespoke containers.
//!
//! Gated behind the `integration` feature, same as `projector_postgres.rs`.
#![cfg(feature = "integration")]

#[path = "support/mod.rs"]
mod support;

use std::time::Duration;

use entity::entities::{account, account_balance, transaction};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::ClientConfig;
use sea_orm::EntityTrait;
use support::{TestKafka, TestPostgres};
use webapp::projection::{run, ProjectorConfig, INGEST_TOPICS};

/// Publishes every fixture record to the broker, each on its manifest topic
/// at partition 0 (the only partition any ingest topic has, §2.1), in
/// manifest order — the same order a real from-scratch Redpanda replay would
/// deliver them in.
async fn publish_fixture_corpus(producer: &FutureProducer, entries: &[(String, String, Vec<u8>, rdkafka::message::OwnedHeaders)]) {
    for (topic, key, payload, headers) in entries {
        producer
            .send(
                FutureRecord::to(topic)
                    .key(key)
                    .payload(payload)
                    .headers(headers.clone()),
                Duration::from_secs(10),
            )
            .await
            .unwrap_or_else(|(e, _)| panic!("publishing to {topic} failed: {e}"));
    }
}

#[tokio::test]
async fn replay_over_kafka_projects_the_whole_fixture_corpus() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let db = TestPostgres::start().await;
    let broker = TestKafka::start().await;

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", broker.bootstrap_servers())
        .set("message.timeout.ms", "10000")
        .create()
        .expect("failed to build a producer against the test Kafka broker");

    let entries = support::load_fixture_publish_entries();
    publish_fixture_corpus(&producer, &entries).await;

    // Every ingest topic needs at least one record published before `run`
    // can resolve its high watermark for `--until-caught-up`; the fixture
    // corpus covers all three (§9), so this always holds.
    for topic in INGEST_TOPICS {
        assert!(
            entries.iter().any(|(t, ..)| t == topic),
            "fixture corpus must cover {topic} for --until-caught-up to have a watermark to reach"
        );
    }

    let mut config = ProjectorConfig::new(broker.bootstrap_servers().to_string());
    config.until_caught_up = true;
    config.batch_max_wait = Duration::from_millis(200);

    let first_conn = webapp::db::seaql::init_db(db.database_url())
        .await
        .expect("failed to open a connection to the test Postgres container");
    tokio::time::timeout(Duration::from_secs(60), run(first_conn, config))
        .await
        .expect("run() must finish within the timeout once --until-caught-up is set")
        .expect("run() must complete without error");

    // `run` consumes its `DatabaseConnection` (sea-orm's isn't `Clone` once
    // the `mock` feature is enabled, which `webapp` does), so every
    // subsequent use -- the first round's assertions, and the second round's
    // `run` call below -- opens its own fresh connection pool against the
    // same still-running container via `db.database_url()`.
    let conn = webapp::db::seaql::init_db(db.database_url())
        .await
        .expect("failed to reconnect to the test Postgres container");

    let account_count = account::Entity::find().all(&conn).await.unwrap().len();
    let balance_count = account_balance::Entity::find().all(&conn).await.unwrap().len();
    let tx_count = transaction::Entity::find().all(&conn).await.unwrap().len();

    // The fixture corpus has 56 transaction manifest entries (49 from
    // iteration 1, plus 7 §7 labeling-demo transactions added in iteration 2:
    // ACC1-LABEL-FITNESS-{01,02,03}, ACC1-LABEL-AMAZON-01, ACC1-LABEL-ACME-01,
    // ACC1-LABEL-OVERRIDDEN-01, ACC1-LABEL-SPLIT-01), 6 balances and 2
    // accounts, minus the two records §2.2 says to skip rather than apply:
    // `ACC1-MISSING-ACCOUNT-0001` (full headers, no `source_account_id`) and
    // `ACC1-PHASE1-0001` (a headerless phase-1 record — `source`/`origin`/
    // `schema_version` all take their §2.2 defaults, but `source_account_id`
    // has no default, so §2.2 is explicit that it is "skip + log" there too)
    // — so transactions land two short of the manifest's transaction count,
    // and every balance/account record applies cleanly. The two new
    // `finreport.user-label` fixture entries are not ingest-topic records and
    // do not affect this count.
    assert_eq!(balance_count, 6, "every balance fixture must project");
    assert_eq!(
        tx_count, 54,
        "every transaction fixture but the two source_account_id-less poison records must project"
    );
    assert!(
        account_count >= 2,
        "both real account fixtures, plus any stub accounts the transactions/balances created, must exist"
    );

    // Re-running end to end a second time (simulating a projector restart
    // against the same, already-caught-up broker) must change nothing: the
    // stored offsets already cover every published record, so `run` should
    // see no new messages before its watermarks are already satisfied and
    // return immediately with an unchanged read model.
    let mut second_config = ProjectorConfig::new(broker.bootstrap_servers().to_string());
    second_config.until_caught_up = true;
    second_config.batch_max_wait = Duration::from_millis(200);
    let conn2 = webapp::db::seaql::init_db(db.database_url())
        .await
        .expect("failed to reconnect to the test Postgres container");
    tokio::time::timeout(Duration::from_secs(60), run(conn2, second_config))
        .await
        .expect("second run() must also finish within the timeout")
        .expect("second run() must complete without error");

    let conn3 = webapp::db::seaql::init_db(db.database_url())
        .await
        .expect("failed to reconnect to the test Postgres container");
    let account_count_2 = account::Entity::find().all(&conn3).await.unwrap().len();
    let balance_count_2 = account_balance::Entity::find().all(&conn3).await.unwrap().len();
    let tx_count_2 = transaction::Entity::find().all(&conn3).await.unwrap().len();
    assert_eq!(account_count, account_count_2, "a second caught-up run must not duplicate accounts");
    assert_eq!(balance_count, balance_count_2, "a second caught-up run must not duplicate balances");
    assert_eq!(tx_count, tx_count_2, "a second caught-up run must not duplicate transactions");
}
