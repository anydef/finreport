//! Best-effort publisher for imported bank data.
//!
//! Message values on the account/balance/transaction topics are the raw
//! Comdirect JSON, byte for byte. Nothing is re-serialized on the way through:
//! the bank's payload is the payload. Everything we know *about* the record —
//! which login imported it, when — travels in Kafka headers instead, so the
//! value stays exactly what the API returned.

use std::time::Duration;

use rdkafka::error::KafkaError;
use rdkafka::message::{Header, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::ClientConfig;
use tracing::warn;

/// How long a single publish may block before we give up on it. Short on
/// purpose: the import loop must not stall behind an unreachable broker.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(10);

/// Provenance attached to every published record as Kafka headers.
pub struct RecordMeta<'a> {
    /// Account key of the Comdirect login that imported this (`0`, `1`, ...).
    pub account_key: &'a str,
    /// That login's human-readable label, when one is configured.
    pub account_name: Option<&'a str>,
    /// When the importer fetched the record, RFC 3339.
    pub imported_at: &'a str,
}

impl RecordMeta<'_> {
    fn headers(&self) -> OwnedHeaders {
        let headers = OwnedHeaders::new()
            .insert(Header {
                key: "comdirect_account_key",
                value: Some(self.account_key),
            })
            .insert(Header {
                key: "imported_at",
                value: Some(self.imported_at),
            });

        match self.account_name {
            Some(name) => headers.insert(Header {
                key: "comdirect_account_name",
                value: Some(name),
            }),
            None => headers,
        }
    }
}

pub struct EventPublisher {
    producer: FutureProducer,
}

impl EventPublisher {
    pub fn connect(brokers: &str) -> Result<Self, KafkaError> {
        let producer: FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", brokers)
            // Don't let a queued backlog wedge the importer; the DB write has
            // already happened by then and Postgres is authoritative.
            .set("message.timeout.ms", "10000")
            .set("compression.type", "snappy")
            .create()?;

        Ok(Self { producer })
    }

    /// Publishes one record verbatim. `value` must be the bytes as received
    /// from Comdirect — do not hand this a re-serialized struct.
    pub async fn publish_raw(
        &self,
        topic: &str,
        key: &str,
        value: &[u8],
        meta: &RecordMeta<'_>,
    ) -> Result<(), KafkaError> {
        let headers = meta.headers();
        let record = FutureRecord::to(topic)
            .key(key)
            .payload(value)
            .headers(headers);

        match self.producer.send(record, PUBLISH_TIMEOUT).await {
            Ok(_) => Ok(()),
            Err((e, _)) => Err(e),
        }
    }

    /// Publish, logging failures instead of propagating them. This is the call
    /// the import loop uses: during dual-write a publish failure costs us an
    /// event, not the import.
    pub async fn publish_best_effort(
        &self,
        topic: &str,
        key: &str,
        value: &[u8],
        meta: &RecordMeta<'_>,
    ) {
        if let Err(e) = self.publish_raw(topic, key, value, meta).await {
            warn!(%topic, %key, %e, "failed to publish event; Postgres still has it");
        }
    }
}
