//! Per-account import resume points, kept on a compacted Kafka topic.
//!
//! The importer reads this at startup to learn how far each account got last
//! time, so it can stop paging Comdirect once it reaches transactions it has
//! already published, instead of walking the full history every run.
//!
//! Compaction is what makes this cheap: the topic holds at most one live
//! record per account, so "read it all" is O(accounts), not O(history). See
//! `docs/kafka-migration.md` for the alternatives that were considered.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::error::KafkaError;
use rdkafka::{ClientConfig, Message, Offset, TopicPartitionList};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::TOPIC_IMPORT_WATERMARK;

/// How long to wait for each poll while draining the topic.
const POLL_TIMEOUT: Duration = Duration::from_secs(5);
/// Ceiling on the drain loop, so a misbehaving broker cannot hang startup.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// How far an account has been imported.
///
/// Unlike the bank payloads on the other topics, this record is ours, so it is
/// a normal serialized struct rather than raw passthrough.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Watermark {
    pub account_id: String,
    /// Booking date of the newest transaction published for this account.
    pub last_booking_date: Option<NaiveDate>,
    /// Reference of that transaction. Preferred over the date when matching,
    /// since several transactions can share a booking date.
    pub last_reference: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Reads the whole compacted topic and returns the surviving record per
/// account. An empty or absent topic yields an empty map, which callers must
/// treat as "import everything" — that is the first-run path.
pub fn load_watermarks(brokers: &str) -> Result<HashMap<String, Watermark>, KafkaError> {
    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers)
        // Assignment is manual below; no group coordination happens, but
        // librdkafka still requires the group id to be set.
        .set("group.id", "finreport-importer-watermark")
        .set("enable.auto.commit", "false")
        .set("auto.offset.reset", "earliest")
        .create()?;

    let metadata =
        consumer.fetch_metadata(Some(TOPIC_IMPORT_WATERMARK), Duration::from_secs(10))?;
    let Some(topic) = metadata.topics().first() else {
        warn!(topic = TOPIC_IMPORT_WATERMARK, "topic not found; importing from scratch");
        return Ok(HashMap::new());
    };

    // (partition, high watermark) for every partition that holds anything.
    let mut pending = Vec::new();
    let mut assignment = TopicPartitionList::new();
    for partition in topic.partitions() {
        let (low, high) = consumer.fetch_watermarks(
            TOPIC_IMPORT_WATERMARK,
            partition.id(),
            Duration::from_secs(10),
        )?;
        if low >= high {
            continue;
        }
        assignment.add_partition_offset(
            TOPIC_IMPORT_WATERMARK,
            partition.id(),
            Offset::Beginning,
        )?;
        pending.push((partition.id(), high));
    }

    if pending.is_empty() {
        debug!(topic = TOPIC_IMPORT_WATERMARK, "no watermarks yet");
        return Ok(HashMap::new());
    }

    consumer.assign(&assignment)?;

    let mut watermarks = HashMap::new();
    let deadline = std::time::Instant::now() + DRAIN_TIMEOUT;
    while !pending.is_empty() && std::time::Instant::now() < deadline {
        let Some(message) = consumer.poll(POLL_TIMEOUT) else {
            continue;
        };
        let message = message?;

        match message.payload() {
            // A tombstone retires an account's watermark: forget it and import
            // that account from scratch next run.
            None => {
                if let Some(key) = message.key().and_then(|k| std::str::from_utf8(k).ok()) {
                    watermarks.remove(key);
                }
            }
            Some(payload) => match serde_json::from_slice::<Watermark>(payload) {
                Ok(watermark) => {
                    watermarks.insert(watermark.account_id.clone(), watermark);
                }
                Err(e) => warn!(
                    offset = message.offset(),
                    %e, "skipping unreadable watermark record"
                ),
            },
        }

        // Drop partitions that have been read to their high watermark.
        let position = message.offset() + 1;
        pending.retain(|(id, high)| !(*id == message.partition() && position >= *high));
    }

    if !pending.is_empty() {
        warn!(
            partitions = ?pending,
            "timed out draining watermarks; some accounts will re-import older transactions"
        );
    }

    Ok(watermarks)
}

/// Publishes an account's resume point. Keyed by `account_id` so compaction
/// keeps exactly the newest one per account.
///
/// Best-effort like the rest of the dual-write: losing this costs a re-import
/// of already-published transactions next run, which is idempotent, not a data
/// loss.
pub async fn publish_watermark(
    publisher: &super::producer::EventPublisher,
    watermark: &Watermark,
    meta: &super::producer::RecordMeta<'_>,
) {
    match serde_json::to_vec(watermark) {
        Ok(payload) => {
            publisher
                .publish_best_effort(
                    TOPIC_IMPORT_WATERMARK,
                    &watermark.account_id,
                    &payload,
                    meta,
                )
                .await
        }
        Err(e) => warn!(account_id = %watermark.account_id, %e, "could not encode watermark"),
    }
}
