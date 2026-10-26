//! Event-log publishing for the Postgres → Kafka migration.
//!
//! Phase 1 is a dual-write: every import still writes Postgres exactly as
//! before and additionally publishes to Redpanda. Postgres remains the source
//! of truth, so publishing is best-effort — a broker outage degrades the event
//! log, it must never stop an import.

/// Does this Kafka failure mean "that topic is not there" rather than "the
/// broker could not tell us"?
///
/// The two must not be confused anywhere a missing topic is tolerable. A
/// topic that does not exist holds no records, so there is nothing to wait
/// for or catch up on — and because the ingest topic list lives in code, a
/// newly added topic always exists in the binary before `terraform apply`
/// creates it on the broker. Treating that as a fault turns every topic
/// addition into a deploy-order outage.
///
/// Everything else — broker transport, all-brokers-down, timeout,
/// authentication — means the answer is *unknown*, which is not the same as
/// "nothing there" and must keep failing loudly.
pub fn topic_absent(e: &rdkafka::error::KafkaError) -> bool {
    use rdkafka::error::KafkaError;
    use rdkafka::types::RDKafkaErrorCode::{
        UnknownPartition, UnknownTopic, UnknownTopicOrPartition,
    };
    matches!(
        e,
        KafkaError::MetadataFetch(UnknownTopic | UnknownTopicOrPartition | UnknownPartition)
    )
}

pub mod decimal_compat;
pub mod c24;
pub mod cash;
pub mod envelope;
pub mod events;
pub mod goals;
pub mod insights;
pub mod labeling;
pub mod links;
pub mod producer;
pub mod purchase_orders;
pub mod savings;
pub mod repair;
pub mod scan;
pub mod watermark;

// Topic constants are owned by `envelope` (the frozen §2.2 contract); re-exported
// here so existing call sites (`producer.rs`, `watermark.rs`, `import_transactions.rs`)
// keep compiling unchanged against `webapp::kafka::TOPIC_*`.
pub use envelope::{
    TOPIC_ACCOUNT, TOPIC_ACCOUNT_BALANCE, TOPIC_IMPORT_WATERMARK, TOPIC_TRANSACTION,
};
