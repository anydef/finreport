//! One-shot, group-less scan of a topic from the beginning to its current end.
//!
//! Shared by the `legacy-backfill` binary (backfill and `--repair-headers`
//! modes) and its integration test. A batch read, not a resumable consumer:
//! nothing is committed.

use std::time::Duration;

use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::message::Headers;
use rdkafka::{ClientConfig, Message, Offset, TopicPartitionList};
use tracing::warn;

/// One record read back from a topic. `payload: None` is a tombstone
/// (compaction's deletion marker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedRecord {
    pub key: Option<String>,
    pub payload: Option<Vec<u8>>,
    /// Every header with a value, in wire order, values read as lossy UTF-8.
    pub headers: Vec<(String, String)>,
}

impl ScannedRecord {
    /// First header named `name`, if present.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Reads `topic` to its current end and returns every record seen. Mirrors
/// `kafka::watermark::load_watermarks`'s drain pattern (manual `assign`, no
/// consumer group offsets, read until each partition reaches the high
/// watermark captured at the start).
pub fn scan_topic(
    brokers: &str,
    topic: &str,
) -> Result<Vec<ScannedRecord>, rdkafka::error::KafkaError> {
    const POLL_TIMEOUT: Duration = Duration::from_secs(5);
    const DRAIN_TIMEOUT: Duration = Duration::from_secs(60);

    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers)
        .set("group.id", "finreport-legacy-backfill")
        .set("enable.auto.commit", "false")
        .set("auto.offset.reset", "earliest")
        .create()?;

    let metadata = consumer.fetch_metadata(Some(topic), Duration::from_secs(10))?;
    let Some(topic_metadata) = metadata.topics().first() else {
        warn!(%topic, "topic not found; treating as empty");
        return Ok(Vec::new());
    };

    let mut pending = Vec::new();
    let mut assignment = TopicPartitionList::new();
    for partition in topic_metadata.partitions() {
        let (low, high) =
            consumer.fetch_watermarks(topic, partition.id(), Duration::from_secs(10))?;
        if low >= high {
            continue;
        }
        assignment.add_partition_offset(topic, partition.id(), Offset::Beginning)?;
        pending.push((partition.id(), high));
    }

    if pending.is_empty() {
        return Ok(Vec::new());
    }

    consumer.assign(&assignment)?;

    let mut records = Vec::new();
    let deadline = std::time::Instant::now() + DRAIN_TIMEOUT;
    while !pending.is_empty() && std::time::Instant::now() < deadline {
        let Some(message) = consumer.poll(POLL_TIMEOUT) else {
            continue;
        };
        let message = message?;

        let headers = message
            .headers()
            .map(|headers| {
                (0..headers.count())
                    .filter_map(|idx| {
                        let header = headers.get(idx);
                        header.value.map(|v| {
                            (
                                header.key.to_string(),
                                String::from_utf8_lossy(v).into_owned(),
                            )
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        records.push(ScannedRecord {
            key: message
                .key()
                .map(|k| String::from_utf8_lossy(k).into_owned()),
            payload: message.payload().map(|p| p.to_vec()),
            headers,
        });

        let position = message.offset() + 1;
        pending.retain(|(id, high)| !(*id == message.partition() && position >= *high));
    }

    if !pending.is_empty() {
        warn!(%topic, partitions = ?pending, "timed out draining topic; some identities may be rescanned as missing");
    }

    Ok(records)
}
