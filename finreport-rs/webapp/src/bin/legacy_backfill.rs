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

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::time::Duration;

use chrono::{NaiveDate, TimeZone, Utc};
use dotenv::dotenv;
use entity::entities::{legacy_account, legacy_account_balance, legacy_account_transactions};
use rdkafka::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use sea_orm::{Database, EntityTrait};
use secrecy::ExposeSecret;
use serde::Serialize;
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;
use utils::settings::Settings;
use webapp::kafka::envelope::{
    CURRENT_SCHEMA_VERSION, HEADER_IMPORTED_AT, ORIGIN_LEGACY_BACKFILL, RecordMeta,
    SOURCE_COMDIRECT,
};
use webapp::kafka::producer::EventPublisher;
use webapp::kafka::repair::{RepairReport, repair_headers};
use webapp::kafka::scan::{ScannedRecord, scan_topic};
use webapp::kafka::watermark::{Watermark, load_watermarks};
use webapp::kafka::{
    TOPIC_ACCOUNT, TOPIC_ACCOUNT_BALANCE, TOPIC_IMPORT_WATERMARK, TOPIC_TRANSACTION,
};

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

/// What a run of this binary does, parsed from `std::env::args()`. The
/// default (no arguments) is the legacy-row backfill the module docs
/// describe; `--tombstone-watermarks` instead runs the deploy runbook's
/// watermark-reset step (§3 step 4 — previously a manual `kcat` stand-in,
/// see the runbook's Gaps section).
enum Mode {
    Backfill,
    TombstoneWatermarks { dry_run: bool },
    RepairHeaders { dry_run: bool },
}

/// `--repair-headers` republishes records that were published without
/// `source_account_id` (which the projector drops as poison) with corrected
/// headers and their original value bytes.
const USAGE: &str = "[--tombstone-watermarks [--dry-run] | --repair-headers [--dry-run]]";

/// Parses `std::env::args()` into a [`Mode`]. Anything beyond the
/// recognized shapes is a startup error rather than a silent no-op — same
/// spirit as `projector`'s `until_caught_up_arg`.
fn parse_args<I: Iterator<Item = String>>(mut args: I) -> Result<Mode, String> {
    match args.next().as_deref() {
        None => Ok(Mode::Backfill),
        Some(flag @ ("--tombstone-watermarks" | "--repair-headers")) => {
            let dry_run = match args.next().as_deref() {
                None => false,
                Some("--dry-run") => true,
                Some(other) => {
                    return Err(format!(
                        "unexpected argument {other:?}; usage: {flag} [--dry-run]"
                    ));
                }
            };
            if args.next().is_some() {
                return Err(format!("too many arguments; usage: {flag} [--dry-run]"));
            }
            Ok(if flag == "--repair-headers" {
                Mode::RepairHeaders { dry_run }
            } else {
                Mode::TombstoneWatermarks { dry_run }
            })
        }
        Some(other) => Err(format!("unknown argument {other:?}; usage: {USAGE}")),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mode = parse_args(std::env::args().skip(1)).map_err(|e| {
        error!(%e, "[startup] invalid arguments");
        e
    })?;

    let settings = Settings::from_env()?;
    let brokers = settings.require_kafka_brokers()?.to_string();

    match mode {
        Mode::Backfill => run_backfill(&settings, &brokers).await,
        Mode::TombstoneWatermarks { dry_run } => run_tombstone_watermarks(&brokers, dry_run).await,
        Mode::RepairHeaders { dry_run } => run_repair_headers(&settings, &brokers, dry_run).await,
    }
}

/// The legacy-row backfill described in the module docs: reconstructs
/// `legacy_*` rows as Kafka records on the live ingest topics, skipping any
/// identity already published.
async fn run_backfill(settings: &Settings, brokers: &str) -> Result<(), Box<dyn Error>> {
    let conn = Database::connect(settings.require_database_url()?.expose_secret()).await?;
    let publisher = EventPublisher::connect(brokers)?;

    let accounts = legacy_account::Entity::find().all(&conn).await?;
    let balances = legacy_account_balance::Entity::find().all(&conn).await?;
    let transactions = legacy_account_transactions::Entity::find()
        .all(&conn)
        .await?;
    info!(
        accounts = accounts.len(),
        balances = balances.len(),
        transactions = transactions.len(),
        "[legacy-backfill] loaded legacy rows"
    );

    let existing_account_keys = existing_keys(&scan_topic(brokers, TOPIC_ACCOUNT)?);
    let existing_transaction_keys = existing_keys(&scan_topic(brokers, TOPIC_TRANSACTION)?);
    let existing_balance_identities =
        existing_balance_identities(&scan_topic(brokers, TOPIC_ACCOUNT_BALANCE)?);

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
        info!(
            "[legacy-backfill] nothing to publish — every legacy identity is already on its topic (idempotent re-run)"
        );
    }

    Ok(())
}

// --- Header repair ------------------------------------------------------------

/// Loads the `reference -> account_id` lookup from `legacy_account_transactions`
/// (read-only) and hands off to the repair logic in `webapp::kafka::repair`.
async fn run_repair_headers(
    settings: &Settings,
    brokers: &str,
    dry_run: bool,
) -> Result<(), Box<dyn Error>> {
    let conn = Database::connect(settings.require_database_url()?.expose_secret()).await?;
    let account_by_reference: HashMap<String, String> = legacy_account_transactions::Entity::find()
        .all(&conn)
        .await?
        .into_iter()
        .map(|t| (t.reference, t.account_id))
        .collect();
    info!(
        references = account_by_reference.len(),
        "[repair-headers] loaded legacy transaction lookup"
    );

    let publisher = if dry_run {
        None
    } else {
        Some(EventPublisher::connect(brokers)?)
    };
    let report = repair_headers(brokers, publisher.as_ref(), &account_by_reference).await?;
    log_repair_report(&report, dry_run);
    Ok(())
}

fn log_repair_report(report: &RepairReport, dry_run: bool) {
    for (topic, count) in &report.repaired {
        info!(%topic, count, dry_run, "[repair-headers] repaired");
    }
    let total: usize = report.repaired.iter().map(|(_, c)| c).sum();
    info!(
        repaired = total,
        unrecoverable = report.unrecoverable,
        imported_at_defaulted = report.imported_at_defaulted,
        dry_run,
        "[repair-headers] done"
    );
    if total == 0 && report.unrecoverable == 0 {
        info!("[repair-headers] nothing to repair (idempotent re-run)");
    }
}

// --- Watermark tombstoning (runbook §3 step 4) ------------------------------
//
// Resets every account's resume point so the next import re-walks full
// history and republishes raw bank bytes — needed once the backfill above
// has reconstructed pre-dual-write history, so compaction converges on the
// real payloads rather than leaving them shadowed by whatever the live
// importer already published past that point.

/// Reads the watermark topic and either lists what a tombstone run would
/// touch (`dry_run`) or publishes a null-value record for every live key.
async fn run_tombstone_watermarks(brokers: &str, dry_run: bool) -> Result<(), Box<dyn Error>> {
    let watermarks = load_watermarks(brokers)?;
    let keys = live_watermark_keys(&watermarks);

    if keys.is_empty() {
        info!("[tombstone-watermarks] no live watermark keys found; nothing to do");
        return Ok(());
    }

    if dry_run {
        info!(
            count = keys.len(),
            keys = ?keys,
            "[tombstone-watermarks] dry run — would publish a tombstone for these keys"
        );
        return Ok(());
    }

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", brokers)
        .set("message.timeout.ms", "10000")
        .create()?;

    for key in &keys {
        // No `.payload(...)` call — a record with a key and no value is a
        // Kafka tombstone, exactly what `load_watermarks`/`existing_keys`
        // already treat compaction's deletion marker as.
        let record = FutureRecord::<str, [u8]>::to(TOPIC_IMPORT_WATERMARK).key(key.as_str());
        producer
            .send(record, Duration::from_secs(10))
            .await
            .map_err(|(e, _)| e)?;
        info!(%key, "[tombstone-watermarks] published tombstone");
    }

    info!(count = keys.len(), "[tombstone-watermarks] done");
    Ok(())
}

/// The account keys with a live (non-tombstoned) watermark, sorted for
/// deterministic output. Pure and unit-tested separately from any broker
/// connection — `load_watermarks` is what talks to Kafka; this just reads
/// its result, the same split `missing`/`scan_topic` use above.
fn live_watermark_keys(watermarks: &HashMap<String, Watermark>) -> Vec<String> {
    let mut keys: Vec<String> = watermarks.keys().cloned().collect();
    keys.sort();
    keys
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
    rows.iter()
        .filter(|row| !existing.contains(key_of(row)))
        .collect()
}

// --- Topic scanning (§2.8 "read each target topic to its end") -------------

/// Compacted-topic identity: the key survives iff its newest record on the
/// topic is not a tombstone. Used for `finreport.account` and
/// `finreport.transaction`.
fn existing_keys(records: &[ScannedRecord]) -> HashSet<String> {
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
///
/// The date must be derived exactly the way `webapp::projection` dates each
/// origin, or a re-run could judge an already-published identity "missing"
/// (republishing it, harmless but noisy) or, worse, the reverse. The two
/// origins disagree on where the date lives:
/// - `legacy-backfill`'s own `ReconstructedBalance` payload carries its own
///   `date` field (`webapp::projection::legacy::LegacyMapper::map_balance`
///   reads `balance.date`, never `imported_at`) — checked first here.
/// - the live Comdirect endpoint's `{value, unit}` payload has no date at all
///   (`webapp::projection::comdirect::ComdirectMapper::map_balance` falls
///   back to `imported_at`) — the `imported_at` header, checked second.
fn existing_balance_identities(records: &[ScannedRecord]) -> HashSet<(String, NaiveDate)> {
    let mut identities = HashSet::new();
    for record in records {
        let (Some(key), Some(payload)) = (&record.key, &record.payload) else {
            continue;
        };
        let date = payload_balance_date(payload).or_else(|| {
            record
                .header(HEADER_IMPORTED_AT)
                .and_then(parse_rfc3339_date)
        });
        if let Some(date) = date {
            identities.insert((key.clone(), date));
        }
    }
    identities
}

/// `legacy-backfill`'s own `ReconstructedBalance` payload's `date` field
/// (`YYYY-MM-DD`, matching `LegacyMapper::map_balance`'s own parsing), or
/// `None` for any payload without one — including the live Comdirect
/// `{value, unit}` shape, which this falls through for.
fn payload_balance_date(payload: &[u8]) -> Option<NaiveDate> {
    #[derive(serde::Deserialize)]
    struct PayloadDate {
        date: String,
    }
    serde_json::from_slice::<PayloadDate>(payload)
        .ok()
        .and_then(|p| NaiveDate::parse_from_str(&p.date, "%Y-%m-%d").ok())
}

fn parse_rfc3339_date(value: &str) -> Option<NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_with_no_arguments_runs_the_backfill() {
        assert!(matches!(
            parse_args(std::iter::empty()).unwrap(),
            Mode::Backfill
        ));
    }

    #[test]
    fn parse_args_tombstone_watermarks_defaults_to_publishing() {
        let args = vec!["--tombstone-watermarks".to_string()].into_iter();
        let mode = parse_args(args).unwrap();
        assert!(matches!(mode, Mode::TombstoneWatermarks { dry_run: false }));
    }

    #[test]
    fn parse_args_tombstone_watermarks_dry_run() {
        let args = vec![
            "--tombstone-watermarks".to_string(),
            "--dry-run".to_string(),
        ]
        .into_iter();
        let mode = parse_args(args).unwrap();
        assert!(matches!(mode, Mode::TombstoneWatermarks { dry_run: true }));
    }

    #[test]
    fn parse_args_repair_headers_defaults_to_publishing() {
        let args = vec!["--repair-headers".to_string()].into_iter();
        assert!(matches!(
            parse_args(args).unwrap(),
            Mode::RepairHeaders { dry_run: false }
        ));
    }

    #[test]
    fn parse_args_repair_headers_dry_run() {
        let args = vec!["--repair-headers".to_string(), "--dry-run".to_string()].into_iter();
        assert!(matches!(
            parse_args(args).unwrap(),
            Mode::RepairHeaders { dry_run: true }
        ));
    }

    #[test]
    fn parse_args_repair_headers_rejects_trailing_arguments() {
        let args = vec![
            "--repair-headers".to_string(),
            "--dry-run".to_string(),
            "x".to_string(),
        ]
        .into_iter();
        assert!(parse_args(args).is_err());
    }

    #[test]
    fn parse_args_rejects_unknown_arguments() {
        let args = vec!["--bogus".to_string()].into_iter();
        assert!(parse_args(args).is_err());
    }

    #[test]
    fn parse_args_rejects_trailing_arguments() {
        let args = vec![
            "--tombstone-watermarks".to_string(),
            "--dry-run".to_string(),
            "extra".to_string(),
        ]
        .into_iter();
        assert!(parse_args(args).is_err());
    }

    fn watermark(account_id: &str) -> Watermark {
        Watermark {
            account_id: account_id.to_string(),
            last_booking_date: None,
            last_reference: None,
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn live_watermark_keys_returns_sorted_account_ids() {
        let watermarks: HashMap<String, Watermark> = [
            ("B-2".to_string(), watermark("B-2")),
            ("A-1".to_string(), watermark("A-1")),
        ]
        .into_iter()
        .collect();

        assert_eq!(live_watermark_keys(&watermarks), vec!["A-1", "B-2"]);
    }

    #[test]
    fn live_watermark_keys_is_empty_when_no_watermarks_are_live() {
        assert!(live_watermark_keys(&HashMap::new()).is_empty());
    }

    fn record(key: &str, payload: Option<&str>, imported_at: Option<&str>) -> ScannedRecord {
        ScannedRecord {
            key: Some(key.to_string()),
            payload: payload.map(|p| p.as_bytes().to_vec()),
            headers: imported_at
                .map(|v| vec![(HEADER_IMPORTED_AT.to_string(), v.to_string())])
                .unwrap_or_default(),
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
        assert!(
            !keys.contains("A-1"),
            "a later tombstone must remove the key"
        );
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
    fn existing_balance_identities_keys_by_account_and_date_from_imported_at_when_payload_has_no_date()
     {
        // The live Comdirect `{value, unit}` shape — no `date` field, so this
        // falls back to `imported_at`, matching `ComdirectMapper::map_balance`.
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
    fn existing_balance_identities_prefers_the_payloads_own_date_over_imported_at() {
        // A `legacy-backfill` reconstruction: `imported_at` is only the
        // placeholder import time, but the payload's own `date` field is the
        // real observation date `LegacyMapper::map_balance` reads — that one
        // must win so a re-run's identity matches the first run's.
        let records = vec![record(
            "A-1",
            Some(r#"{"account_id":"A-1","date":"2023-11-30","amount":1.0}"#),
            Some("2024-06-15T00:00:00+00:00"),
        )];
        let identities = existing_balance_identities(&records);

        let nov_30 = NaiveDate::from_ymd_opt(2023, 11, 30).unwrap();
        let jun_15 = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        assert!(identities.contains(&("A-1".to_string(), nov_30)));
        assert!(!identities.contains(&("A-1".to_string(), jun_15)));
        assert_eq!(identities.len(), 1);
    }

    #[test]
    fn existing_balance_identities_ignores_records_with_no_date_anywhere() {
        // Can't happen on the real topic (the importer always sets
        // `imported_at`), but the scanner must not panic or silently mis-key
        // on one that somehow lacks both.
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
        assert_eq!(
            first_run, second_run,
            "a re-run must make the same decision"
        );
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
