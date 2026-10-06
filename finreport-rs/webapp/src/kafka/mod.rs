//! Event-log publishing for the Postgres → Kafka migration.
//!
//! Phase 1 is a dual-write: every import still writes Postgres exactly as
//! before and additionally publishes to Redpanda. Postgres remains the source
//! of truth, so publishing is best-effort — a broker outage degrades the event
//! log, it must never stop an import.

pub mod envelope;
pub mod events;
pub mod producer;
pub mod watermark;

// Topic constants are owned by `envelope` (the frozen §2.2 contract); re-exported
// here so existing call sites (`producer.rs`, `watermark.rs`, `import_transactions.rs`)
// keep compiling unchanged against `webapp::kafka::TOPIC_*`.
pub use envelope::{
    TOPIC_ACCOUNT, TOPIC_ACCOUNT_BALANCE, TOPIC_IMPORT_WATERMARK, TOPIC_TRANSACTION,
};

