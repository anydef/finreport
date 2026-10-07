//! Republish-then-idempotent-rerun cycle for `legacy-backfill --repair-headers`
//! against a throwaway Kafka broker. Gated behind the `integration` feature.
#![cfg(feature = "integration")]

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::time::Duration;

use rdkafka::ClientConfig;
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord};
use support::TestKafka;
use webapp::kafka::envelope::{TOPIC_ACCOUNT_BALANCE, TOPIC_TRANSACTION};
use webapp::kafka::producer::EventPublisher;
use webapp::kafka::repair::{RepairReport, repair_headers};
use webapp::kafka::scan::scan_topic;

const RAW: &str = "{ \"reference\" : \"R/1\",  \"amount\":1 }";

fn count(report: &RepairReport, topic: &str) -> usize {
    report.repaired.iter().find(|(t, _)| *t == topic).unwrap().1
}

#[tokio::test]
async fn repair_republishes_with_original_bytes_then_a_rerun_finds_nothing() {
    let broker = TestKafka::start().await;
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", broker.bootstrap_servers())
        .set("message.timeout.ms", "10000")
        .create()
        .unwrap();

    let broken = |imported: &str| {
        OwnedHeaders::new()
            .insert(Header {
                key: "comdirect_account_key",
                value: Some("0"),
            })
            .insert(Header {
                key: "imported_at",
                value: Some(imported),
            })
    };
    for (topic, key, payload) in [
        (TOPIC_TRANSACTION, "R/1", RAW),
        (TOPIC_TRANSACTION, "R/orphan", "{}"),
        (TOPIC_ACCOUNT_BALANCE, "ACC-1", "{\"value\":\"1\"}"),
    ] {
        producer
            .send(
                FutureRecord::to(topic)
                    .key(key)
                    .payload(payload)
                    .headers(broken("2026-09-28T20:17:17+00:00")),
                Duration::from_secs(10),
            )
            .await
            .unwrap_or_else(|(e, _)| panic!("{e}"));
    }

    let lookup: HashMap<String, String> = [("R/1".to_string(), "ACC-1".to_string())].into();
    let brokers = broker.bootstrap_servers().to_string();

    // Dry run publishes nothing.
    let dry = repair_headers(&brokers, None, &lookup).await.unwrap();
    assert_eq!(count(&dry, TOPIC_TRANSACTION), 1);
    assert_eq!(dry.unrecoverable, 1);
    assert_eq!(scan_topic(&brokers, TOPIC_TRANSACTION).unwrap().len(), 2);

    let publisher = EventPublisher::connect(&brokers).unwrap();
    let first = repair_headers(&brokers, Some(&publisher), &lookup)
        .await
        .unwrap();
    assert_eq!(count(&first, TOPIC_TRANSACTION), 1);
    assert_eq!(count(&first, TOPIC_ACCOUNT_BALANCE), 1);
    assert_eq!(first.unrecoverable, 1);

    let tx = scan_topic(&brokers, TOPIC_TRANSACTION).unwrap();
    let repaired = tx
        .iter()
        .rev()
        .find(|r| r.key.as_deref() == Some("R/1"))
        .unwrap();
    assert_eq!(
        repaired.payload.as_deref(),
        Some(RAW.as_bytes()),
        "value bytes must be untouched"
    );
    assert_eq!(repaired.header("source_account_id"), Some("ACC-1"));
    assert_eq!(repaired.header("origin"), Some("source"));
    assert_eq!(
        repaired.header("imported_at"),
        Some("2026-09-28T20:17:17+00:00")
    );

    // Second run: only the unrecoverable orphan remains; nothing is repaired.
    let second = repair_headers(&brokers, Some(&publisher), &lookup)
        .await
        .unwrap();
    assert_eq!(count(&second, TOPIC_TRANSACTION), 0);
    assert_eq!(count(&second, TOPIC_ACCOUNT_BALANCE), 0);
    assert_eq!(second.unrecoverable, 1);
}
