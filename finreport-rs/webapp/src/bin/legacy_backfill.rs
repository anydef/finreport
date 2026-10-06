//! One-off migration of the renamed `legacy_*` tables onto the regular
//! ingest topics, tagged `origin=legacy-backfill` (§2.8).
//!
//! Reconstructs Comdirect-shaped history that predates the event log (no raw
//! payload was ever kept for it) as our own JSON, under the *live* keys the
//! raw bank data uses (`account_id` / `reference`), and publishes only
//! identities the target topic does not already carry. That key-skipping is
//! what makes every re-run a no-op: a first run's own output is what a
//! second run sees as "already there".
//!
//! Read-only against Postgres — this binary never writes `legacy_*` or any
//! other table, only publishes to Kafka.

use std::collections::HashSet;
use std::error::Error;
use std::time::Duration;

use chrono::{NaiveDate, TimeZone, Utc};
use dotenv::dotenv;
use entity::entities::{legacy_account, legacy_account_balance, legacy_account_transactions};
use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::message::Headers;
use rdkafka::{ClientConfig, Message, Offset, TopicPartitionList};
use sea_orm::{Database, EntityTrait};
use secrecy::ExposeSecret;
use serde::Serialize;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;
use utils::settings::Settings;
use webapp::kafka::envelope::{
    RecordMeta, CURRENT_SCHEMA_VERSION, HEADER_IMPORTED_AT, ORIGIN_LEGACY_BACKFILL,
    SOURCE_COMDIRECT,
};
use webapp::kafka::producer::EventPublisher;
use webapp::kafka::{TOPIC_ACCOUNT, TOPIC_ACCOUNT_BALANCE, TOPIC_TRANSACTION};

/// `comdirect_account_key` is a config key (`"0"`, `"1"`, ...) that names
/// which login imported a *live* record. `legacy_*` rows predate that
/// bookkeeping entirely — there is no login to attribute them to — so this
/// sentinel fills the header's required slot without claiming to be a real
/// account key. Distinct from the `origin` header value `legacy-backfill` so
/// the two concerns (producer identity vs. this specific sentinel) can't be
/// confused in a log line.
const LEGACY_BACKFILL_ACCOUNT_KEY: &str = "legacy-backfill-reconstruction";

/// `legacy_account` carries no timestamp of its own (it was a point-in-time
/// upsert target, not an event). Every reconstructed `account` record uses
/// this fixed placeholder so repeated backfills are byte-identical; nothing
/// orders on it since `finreport.account` is compacted by key, not by time.
const ACCOUNT_IMPORTED_AT_PLACEHOLDER: &str = "1970-01-01T00:00:00Z";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let settings = Settings::from_env()?;
    let brokers = settings.require_kafka_brokers()?.to_string();
    let conn = Database::connect(settings.require_database_url()?.expose_secret()).await?;
    let publisher = EventPublisher::connect(&brokers)?;

    let accounts = legacy_account::Entity::find().all(&conn).await?;
    let balances = legacy_account_balance::Entity::find().all(&conn).await?;
    let transactions = legacy_account_transactions::Entity::find().all(&conn).await?;
    info!(
        accounts = accounts.len(),
        balances = balances.len(),
        transactions = transactions.len(),
        "[legacy-backfill] loaded legacy rows"
    );

    let existing_account_keys = existing_keys(&scan_topic(&brokers, TOPIC_ACCOUNT)?);
    let existing_transaction_keys = existing_keys(&scan_topic(&brokers, TOPIC_TRANSACTION)?);
    let existing_balance_identities =
        existing_balance_identities(&scan_topic(&brokers, TOPIC_ACCOUNT_BALANCE)?);

    let mut published = 0u32;

    let missing_accounts = missing(&accounts, |a| &a.account_id, &existing_account_keys);
    for account in &missing_accounts {
        let payload = serde_json::to_vec(&ReconstructedAccount::from(*account))?;
        let meta = account_record_meta(account);
        publisher
            .publish(TOPIC_ACCOUNT, &account.account_id, &payload, &meta)
            .await?;
        published += 1;
        debug!(account_id = %account.account_id, "published legacy account");
    }

    let missing_balances: Vec<&legacy_account_balance::Model> = balances
        .iter()
        .filter(|balance| {
            !existing_balance_identities.contains(&(balance.account_id.clone(), balance.date))
        })
        .collect();
    for balance in &missing_balances {
        let payload = serde_json::to_vec(&ReconstructedBalance::from(*balance))?;
        let meta = balance_record_meta(balance);
        publisher
            .publish(TOPIC_ACCOUNT_BALANCE, &balance.account_id, &payload, &meta)
            .await?;
        published += 1;
        debug!(account_id = %balance.account_id, date = %balance.date, "published legacy balance");
    }

    let missing_transactions = missing(&transactions, |t| &t.reference, &existing_transaction_keys);
    for transaction in &missing_transactions {
        let payload = serde_json::to_vec(&ReconstructedTransaction::from(*transaction))?;
        let meta = transaction_record_meta(transaction);
        publisher
            .publish(TOPIC_TRANSACTION, &transaction.reference, &payload, &meta)
            .await?;
        published += 1;
        debug!(reference = %transaction.reference, "published legacy transaction");
    }

    let skipped = (accounts.len() + balances.len() + transactions.len()) as u32 - published;
    info!(published, skipped, "[legacy-backfill] done");
    if published == 0 {
        info!("[legacy-backfill] nothing to publish — every legacy identity is already on its topic (idempotent re-run)");
    }

    Ok(())
}

// --- Reconstructed payload shapes -------------------------------------------
//
// There is no bank payload to forward for pre-dual-write history, so these
// are our own JSON — a straight serialization of the `legacy_*` columns,
// matching `fixtures/payloads/legacy/transaction-0001.json` (§2.8, the shape
// WP0 already committed as the reference for this reconstruction).

#[derive(Serialize)]
struct ReconstructedAccount<'a> {
    account_id: &'a str,
    display_id: &'a str,
    account_type: &'a str,
    iban: &'a str,
    bic: &'a str,
    institute: &'a str,
    account_name: Option<&'a str>,
}

impl<'a> From<&'a legacy_account::Model> for ReconstructedAccount<'a> {
    fn from(row: &'a legacy_account::Model) -> Self {
        ReconstructedAccount {
            account_id: &row.account_id,
            display_id: &row.display_id,
            account_type: &row.account_type,
            iban: &row.iban,
            bic: &row.bic,
            institute: &row.institute,
            account_name: row.account_name.as_deref(),
        }
    }
}

#[derive(Serialize)]
struct ReconstructedBalance<'a> {
    account_id: &'a str,
    date: NaiveDate,
    amount: f64,
}

impl<'a> From<&'a legacy_account_balance::Model> for ReconstructedBalance<'a> {
    fn from(row: &'a legacy_account_balance::Model) -> Self {
        ReconstructedBalance {
            account_id: &row.account_id,
            date: row.date,
            amount: row.amount,
        }
    }
}

#[derive(Serialize)]
struct ReconstructedTransaction<'a> {
    reference: &'a str,
    account_id: &'a str,
    booking_status: &'a str,
    booking_date: NaiveDate,
    amount: f64,
    remitter: &'a str,
    deptor: &'a str,
    creditor: &'a str,
    creditor_id: &'a str,
    creditor_mandate_id: &'a str,
    remittance_info: &'a str,
    transaction_type: &'a str,
}

impl<'a> From<&'a legacy_account_transactions::Model> for ReconstructedTransaction<'a> {
    fn from(row: &'a legacy_account_transactions::Model) -> Self {
        ReconstructedTransaction {
            reference: &row.reference,
            account_id: &row.account_id,
            booking_status: &row.booking_status,
            booking_date: row.booking_date,
            amount: row.amount,
            remitter: &row.remitter,
            deptor: &row.deptor,
            creditor: &row.creditor,
            creditor_id: &row.creditor_id,
            creditor_mandate_id: &row.creditor_mandate_id,
            remittance_info: &row.remittance_info,
            transaction_type: &row.transaction_type,
        }
    }
}

// --- Headers -----------------------------------------------------------------

fn account_record_meta(row: &legacy_account::Model) -> RecordMeta<'_> {
    RecordMeta {
        source: SOURCE_COMDIRECT,
        source_account_id: Some(&row.account_id),
        origin: ORIGIN_LEGACY_BACKFILL,
        schema_version: CURRENT_SCHEMA_VERSION,
        imported_at: ACCOUNT_IMPORTED_AT_PLACEHOLDER,
        comdirect_account_key: LEGACY_BACKFILL_ACCOUNT_KEY,
        comdirect_account_name: row.account_name.as_deref(),
    }
}

fn balance_record_meta(row: &legacy_account_balance::Model) -> RecordMeta<'_> {
    // Leaked once per record rather than threaded through as an owned String
    // on the caller's stack: this binary is a short-lived one-off batch job,
    // not a long-running process, so the allocation is bounded by the
    // (small, one-off) row count and never repeats across runs.
    let imported_at: &'static str = Box::leak(midnight_utc_rfc3339(row.date).into_boxed_str());
    RecordMeta {
        source: SOURCE_COMDIRECT,
        source_account_id: Some(&row.account_id),
        origin: ORIGIN_LEGACY_BACKFILL,
        schema_version: CURRENT_SCHEMA_VERSION,
        imported_at,
        comdirect_account_key: LEGACY_BACKFILL_ACCOUNT_KEY,
        comdirect_account_name: None,
    }
}

fn transaction_record_meta(row: &legacy_account_transactions::Model) -> RecordMeta<'_> {
    let imported_at: &'static str =
        Box::leak(midnight_utc_rfc3339(row.booking_date).into_boxed_str());
    RecordMeta {
        source: SOURCE_COMDIRECT,
        source_account_id: Some(&row.account_id),
        origin: ORIGIN_LEGACY_BACKFILL,
        schema_version: CURRENT_SCHEMA_VERSION,
        imported_at,
        comdirect_account_key: LEGACY_BACKFILL_ACCOUNT_KEY,
        comdirect_account_name: None,
    }
}

fn midnight_utc_rfc3339(date: NaiveDate) -> String {
    Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight is always valid"))
        .to_rfc3339()
}

/// Rows whose identity (as extracted by `key_of`) is not already in
/// `existing`. Pure and unit-tested independently of any broker connection —
/// the actual skip decision lives here, scanning the topic is just how
/// `existing` gets populated at runtime.
fn missing<'a, T>(
    rows: &'a [T],
    key_of: impl Fn(&T) -> &str,
    existing: &HashSet<String>,
) -> Vec<&'a T> {
    rows.iter().filter(|row| !existing.contains(key_of(row))).collect()
}

// --- Topic scanning (§2.8 "read each target topic to its end") -------------

/// One record read back from a target topic while determining what is
/// already there. `payload: None` is a tombstone (compaction's deletion
/// marker) — `existing_keys`/`existing_balance_identities` both treat it as
/// "this identity is gone", not "this identity exists with an empty value".
struct ExistingRecord {
    key: Option<String>,
    payload: Option<Vec<u8>>,
    imported_at_header: Option<String>,
}

/// Reads `topic` to its current end and returns every record seen. Mirrors
/// `webapp::kafka::watermark::load_watermarks`'s drain pattern (manual
/// `assign`, no consumer group, read until each partition's offset reaches
/// the high watermark captured at the start) — this is a one-off batch read,
/// not a resumable consumer, so there is nothing to commit.
fn scan_topic(
    brokers: &str,
    topic: &str,
) -> Result<Vec<ExistingRecord>, rdkafka::error::KafkaError> {
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
        let (low, high) = consumer.fetch_watermarks(topic, partition.id(), Duration::from_secs(10))?;
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

        let key = message
            .key()
            .map(|k| String::from_utf8_lossy(k).into_owned());
        let payload = message.payload().map(|p| p.to_vec());
        let imported_at_header = message.headers().and_then(|headers| {
            (0..headers.count()).find_map(|idx| {
                let header = headers.get(idx);
                (header.key == HEADER_IMPORTED_AT)
                    .then(|| header.value.map(|v| String::from_utf8_lossy(v).into_owned()))
                    .flatten()
            })
        });

        records.push(ExistingRecord {
            key,
            payload,
            imported_at_header,
        });

        let position = message.offset() + 1;
        pending.retain(|(id, high)| !(*id == message.partition() && position >= *high));
    }

    if !pending.is_empty() {
        warn!(%topic, partitions = ?pending, "timed out draining topic; some identities may be rescanned as missing");
    }

    Ok(records)
}

/// Compacted-topic identity: the key survives iff its newest record on the
/// topic is not a tombstone. Used for `finreport.account` and
/// `finreport.transaction`.
fn existing_keys(records: &[ExistingRecord]) -> HashSet<String> {
    let mut keys = HashSet::new();
    for record in records {
        let Some(key) = &record.key else { continue };
        match &record.payload {
            Some(_) => {
                keys.insert(key.clone());
            }
            None => {
                keys.remove(key);
            }
        }
    }
    keys
}

/// `finreport.account-balance` is delete-cleanup, not compacted — every
/// observation under a key persists, so "already there" has to mean "this
/// exact (account, date) observation", not "this key has ever appeared".
/// The date comes from the `imported_at` header (§2.5: balance payloads carry
/// no date of their own), parsed to a calendar date in UTC.
fn existing_balance_identities(records: &[ExistingRecord]) -> HashSet<(String, NaiveDate)> {
    let mut identities = HashSet::new();
    for record in records {
        let (Some(key), Some(_), Some(imported_at)) =
            (&record.key, &record.payload, &record.imported_at_header)
        else {
            continue;
        };
        if let Some(date) = parse_rfc3339_date(imported_at) {
            identities.insert((key.clone(), date));
        }
    }
    identities
}

fn parse_rfc3339_date(value: &str) -> Option<NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(key: &str, payload: Option<&str>, imported_at: Option<&str>) -> ExistingRecord {
        ExistingRecord {
            key: Some(key.to_string()),
            payload: payload.map(|p| p.as_bytes().to_vec()),
            imported_at_header: imported_at.map(str::to_string),
        }
    }

    #[test]
    fn existing_keys_collects_every_live_key() {
        let records = vec![
            record("A-1", Some("{}"), None),
            record("A-2", Some("{}"), None),
        ];
        let keys = existing_keys(&records);
        assert!(keys.contains("A-1"));
        assert!(keys.contains("A-2"));
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn existing_keys_forgets_a_tombstoned_key() {
        let records = vec![record("A-1", Some("{}"), None), record("A-1", None, None)];
        let keys = existing_keys(&records);
        assert!(!keys.contains("A-1"), "a later tombstone must remove the key");
    }

    #[test]
    fn missing_skips_rows_whose_key_already_exists() {
        #[derive(Debug, PartialEq)]
        struct Row(&'static str);

        let rows = vec![Row("A-1"), Row("A-2"), Row("A-3")];
        let existing: HashSet<String> = ["A-2".to_string()].into_iter().collect();

        let left = missing(&rows, |r| r.0, &existing);

        assert_eq!(left, vec![&Row("A-1"), &Row("A-3")]);
    }

    #[test]
    fn existing_balance_identities_keys_by_account_and_date_from_imported_at() {
        let records = vec![
            record("A-1", Some("{}"), Some("2024-01-31T06:00:00Z")),
            record("A-1", Some("{}"), Some("2024-03-31T06:00:00Z")),
            record("A-2", Some("{}"), Some("2024-01-31T06:00:00Z")),
        ];
        let identities = existing_balance_identities(&records);

        let jan_31 = NaiveDate::from_ymd_opt(2024, 1, 31).unwrap();
        let mar_31 = NaiveDate::from_ymd_opt(2024, 3, 31).unwrap();
        assert!(identities.contains(&("A-1".to_string(), jan_31)));
        assert!(identities.contains(&("A-1".to_string(), mar_31)));
        assert!(identities.contains(&("A-2".to_string(), jan_31)));
        assert!(!identities.contains(&("A-2".to_string(), mar_31)));
        assert_eq!(identities.len(), 3);
    }

    #[test]
    fn existing_balance_identities_ignores_records_with_no_imported_at() {
        // Can't happen on the real topic (the importer always sets it), but
        // the scanner must not panic or silently mis-key on one that somehow
        // lacks it.
        let records = vec![record("A-1", Some("{}"), None)];
        assert!(existing_balance_identities(&records).is_empty());
    }

    #[test]
    fn a_second_scan_of_the_same_identities_changes_nothing() {
        let rows = vec![legacy_account::Model {
            id: 1,
            account_id: "A-1".to_string(),
            display_id: "disp".to_string(),
            account_type: "Girokonto".to_string(),
            iban: "DE00".to_string(),
            bic: "COBADEFFXXX".to_string(),
            institute: "COMDIRECT".to_string(),
            account_name: None,
        }];
        let existing: HashSet<String> = ["A-1".to_string()].into_iter().collect();

        let first_run = missing(&rows, |a| a.account_id.as_str(), &existing);
        let second_run = missing(&rows, |a| a.account_id.as_str(), &existing);

        assert!(first_run.is_empty(), "already-present key must be skipped");
        assert_eq!(first_run, second_run, "a re-run must make the same decision");
    }

    #[test]
    fn midnight_utc_rfc3339_is_deterministic() {
        let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        assert_eq!(midnight_utc_rfc3339(date), midnight_utc_rfc3339(date));
        assert_eq!(midnight_utc_rfc3339(date), "2024-06-15T00:00:00+00:00");
    }

    #[test]
    fn reconstructed_transaction_matches_the_committed_legacy_fixture_shape() {
        let row = legacy_account_transactions::Model {
            id: 1,
            reference: "LEGACY-0001".to_string(),
            account_id: "DE1053820100A1".to_string(),
            booking_status: "BOOKED".to_string(),
            booking_date: NaiveDate::from_ymd_opt(2023, 11, 15).unwrap(),
            amount: -54.3,
            remitter: String::new(),
            deptor: String::new(),
            creditor: "Alte Apotheke".to_string(),
            creditor_id: String::new(),
            creditor_mandate_id: String::new(),
            remittance_info: "Rezept".to_string(),
            transaction_type: "CARD_PAYMENT".to_string(),
        };

        let json = serde_json::to_value(ReconstructedTransaction::from(&row)).unwrap();
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../fixtures/payloads/legacy/transaction-0001.json"
        ))
        .unwrap();

        assert_eq!(json, fixture);
    }
}
