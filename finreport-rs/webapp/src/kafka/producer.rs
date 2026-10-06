//! Publisher for the event log (§2.6).
//!
//! Kafka is now the importer's only output: there is no Postgres write to
//! fall back on, so a publish failure is data loss, not a degraded
//! side-channel. `publish` surfaces that failure as a `Result` for callers
//! that must react to it (the transaction loop skips advancing the
//! account's watermark on failure, §2.6/§2.7); `publish_best_effort` stays
//! available for callers content to log and move on (account/balance
//! snapshots, which the next import cycle naturally re-publishes).
//!
//! Message values on the account/balance/transaction topics are the raw
//! Comdirect JSON, byte for byte. Nothing is re-serialized on the way
//! through: the bank's payload is the payload. Everything we know *about*
//! the record travels in the §2.2 envelope headers instead, built by
//! `super::envelope::RecordMeta` — never hand this a re-serialized struct.

use std::time::Duration;

use rdkafka::error::KafkaError;
use rdkafka::message::OwnedHeaders;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::ClientConfig;
use tracing::warn;

use super::envelope::RecordMeta;

/// How long a single publish may block before we give up on it. Short on
/// purpose: the import loop must not stall behind an unreachable broker.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(10);

pub struct EventPublisher {
    producer: FutureProducer,
}

impl EventPublisher {
    pub fn connect(brokers: &str) -> Result<Self, KafkaError> {
        let producer: FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", brokers)
            .set("message.timeout.ms", "10000")
            .set("compression.type", "snappy")
            .create()?;

        Ok(Self { producer })
    }

    /// Publishes one record verbatim with the given headers. `value` must be
    /// the bytes as received from the source — do not hand this a
    /// re-serialized struct. The lowest-level primitive; `publish` and
    /// `publish_best_effort` build their headers from a `RecordMeta` and call
    /// this.
    pub async fn publish_with_headers(
        &self,
        topic: &str,
        key: &str,
        value: &[u8],
        headers: OwnedHeaders,
    ) -> Result<(), KafkaError> {
        let record = FutureRecord::to(topic)
            .key(key)
            .payload(value)
            .headers(headers);

        match self.producer.send(record, PUBLISH_TIMEOUT).await {
            Ok(_) => Ok(()),
            Err((e, _)) => Err(e),
        }
    }

    /// Publishes one record verbatim, propagating a failure instead of
    /// swallowing it. Used wherever losing the publish must be visible to the
    /// caller — the transaction loop skips advancing the watermark on `Err`
    /// (§2.6/§2.7).
    pub async fn publish(
        &self,
        topic: &str,
        key: &str,
        value: &[u8],
        meta: &RecordMeta<'_>,
    ) -> Result<(), KafkaError> {
        self.publish_with_headers(topic, key, value, meta.headers())
            .await
    }

    /// Publish, logging failures instead of propagating them. Used for
    /// records whose loss is self-healing (the next import cycle republishes
    /// the account/balance snapshot it describes), unlike a transaction,
    /// which exists exactly once in the bank's history.
    pub async fn publish_best_effort(
        &self,
        topic: &str,
        key: &str,
        value: &[u8],
        meta: &RecordMeta<'_>,
    ) {
        if let Err(e) = self.publish(topic, key, value, meta).await {
            warn!(%topic, %key, %e, "failed to publish event");
        }
    }
}
