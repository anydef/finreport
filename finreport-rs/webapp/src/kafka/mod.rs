//! Event-log publishing for the Postgres → Kafka migration.
//!
//! Phase 1 is a dual-write: every import still writes Postgres exactly as
//! before and additionally publishes to Redpanda. Postgres remains the source
//! of truth, so publishing is best-effort — a broker outage degrades the event
//! log, it must never stop an import.

pub mod events;
pub mod producer;
pub mod watermark;

/// Account entity snapshots. Compacted: the latest record per account wins.
pub const TOPIC_ACCOUNT: &str = "finreport.account";
/// Balance observations, one per account per import. Retained forever.
pub const TOPIC_ACCOUNT_BALANCE: &str = "finreport.account-balance";
/// Transaction events. Retained forever.
pub const TOPIC_TRANSACTION: &str = "finreport.transaction";
/// Per-account import resume points. Compacted.
pub const TOPIC_IMPORT_WATERMARK: &str = "finreport.import-watermark";
