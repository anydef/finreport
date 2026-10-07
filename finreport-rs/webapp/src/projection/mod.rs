//! Kafka-to-Postgres projector: per-source mappers and the `projector` batch
//! loop (§2.3, §2.4).
//!
//! [`process_batch`] is the pure half (map → upsert → commit offsets, all in
//! one DB transaction, no Kafka I/O) and is what the integration tests drive
//! directly; [`run`] is the thin, hard-to-unit-test half that polls a
//! `StreamConsumer` and feeds it batches.

pub mod comdirect;
pub mod labeling;
pub mod legacy;
pub mod mapper;
pub mod offsets;
pub mod records;
pub mod upsert;

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::{BorrowedMessage, Message};
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use rdkafka::{ClientConfig, Timestamp};
use sea_orm::{DatabaseConnection, DatabaseTransaction, DbErr, TransactionTrait};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::kafka::envelope::{
    transaction_uuid, Envelope, SourceEvent, TOPIC_ACCOUNT, TOPIC_ACCOUNT_BALANCE,
    TOPIC_TRANSACTION,
};
use crate::kafka::labeling::{
    CacheRecord, CategoryRecord, LabelRecord, RuleRecord, UserLabelRecord,
};
use mapper::{MapperInput, MapperRegistry, SourceMapper};
use records::MapError;

// ---------------------------------------------------------------------------
// Tuning (§2.3)
// ---------------------------------------------------------------------------

/// Upper bound on records folded into one DB transaction.
pub const DEFAULT_BATCH_MAX_RECORDS: usize = 500;
/// How long `run` waits for `DEFAULT_BATCH_MAX_RECORDS` to fill before
/// flushing a smaller batch anyway — keeps latency bounded on a quiet topic.
pub const DEFAULT_BATCH_MAX_WAIT: Duration = Duration::from_millis(500);
/// After this many consecutive batch-write failures `run` gives up and
/// returns an error, handing the restart decision to the process supervisor
/// (§2.3) — a DB outage is retryable, a mapping bug is not, and this bound is
/// what tells the two apart without a human watching.
pub const DEFAULT_MAX_CONSECUTIVE_WRITE_FAILURES: u32 = 5;

/// The three ingest topics the projector consumes — **not**
/// `finreport.import-watermark`, which is importer-private (§2.3). All three
/// are single-partition (§2.1's ordering guarantee), so partition 0 is the
/// only partition that ever exists.
pub const INGEST_TOPICS: [&str; 3] = [TOPIC_ACCOUNT, TOPIC_ACCOUNT_BALANCE, TOPIC_TRANSACTION];
const INGEST_PARTITION: i32 = 0;

/// The five labeling *output* topics ([`labeling_topic_for`]) this projector
/// also consumes and projects, alongside [`INGEST_TOPICS`] — same batch,
/// same transaction, same offset bookkeeping (unsuffixed keys, like the
/// ingest topics; distinct from the labeler's own `@labeler`-suffixed
/// bookkeeping of *its* four input topics). `category`/`rule`/`user-label`
/// are already self-projected by their writers (`category_seed`, the
/// labeler's own consume loop), so this is belt-and-braces (idempotent
/// upserts) for those three — it is load-bearing for `transaction-label`,
/// which only the labeler itself wrote before, under an offset key nothing
/// ever advanced.
pub const LABELING_PROJECTION_TOPICS: [&str; 5] = [
    crate::kafka::labeling::TOPIC_CATEGORY,
    crate::kafka::labeling::TOPIC_TRANSACTION_LABEL,
    crate::kafka::labeling::TOPIC_LLM_CACHE,
    crate::kafka::labeling::TOPIC_USER_LABEL,
    crate::kafka::labeling::TOPIC_RULE,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntityKind {
    Account,
    Balance,
    Transaction,
}

fn entity_kind_for_topic(topic: &str) -> Option<EntityKind> {
    match topic {
        TOPIC_ACCOUNT => Some(EntityKind::Account),
        TOPIC_ACCOUNT_BALANCE => Some(EntityKind::Balance),
        TOPIC_TRANSACTION => Some(EntityKind::Transaction),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Labeling-topic dispatch (§2.2/§3, frozen by WP0 — see
// `webapp/src/projection/labeling.rs`)
// ---------------------------------------------------------------------------
//
// This is **not** wired into [`process_batch`]/[`run`] above: those two only
// ever see [`INGEST_TOPICS`]. The forthcoming `labeler` binary (WP3) is the
// intended caller, once it has its own consume loop over
// `finreport.{transaction,user-label,rule,label-request}` (§2.3) and needs to
// know which `projection::labeling` function projects each of the five
// *output* topics it (and `category-seed`) write to.

/// One of the five new topics this crate projects into a Postgres table —
/// as distinct from [`EntityKind`], which is the three original ingest
/// topics the `projector` binary consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelingTopic {
    Category,
    TransactionLabel,
    LlmCache,
    UserLabel,
    Rule,
}

/// Maps a §2.2 topic name to the [`LabelingTopic`] WP3's projector dispatches
/// it to. `finreport.label-request` is deliberately absent: it is a work
/// queue the labeler consumes to decide what to re-resolve, not a topic any
/// `projection::labeling` function projects.
pub fn labeling_topic_for(topic: &str) -> Option<LabelingTopic> {
    use crate::kafka::labeling::{
        TOPIC_CATEGORY, TOPIC_LLM_CACHE, TOPIC_RULE, TOPIC_TRANSACTION_LABEL, TOPIC_USER_LABEL,
    };
    match topic {
        TOPIC_CATEGORY => Some(LabelingTopic::Category),
        TOPIC_TRANSACTION_LABEL => Some(LabelingTopic::TransactionLabel),
        TOPIC_LLM_CACHE => Some(LabelingTopic::LlmCache),
        TOPIC_USER_LABEL => Some(LabelingTopic::UserLabel),
        TOPIC_RULE => Some(LabelingTopic::Rule),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The pure half
// ---------------------------------------------------------------------------

/// One decoded ingest record, detached from the broker connection so batches
/// can be assembled, replayed and asserted on in tests without a live
/// consumer.
#[derive(Debug, Clone)]
pub struct ConsumedRecord {
    pub topic: String,
    pub partition: i32,
    pub offset: i64,
    pub key: Option<String>,
    pub payload: Vec<u8>,
    pub envelope: Envelope,
}

impl ConsumedRecord {
    fn from_message(message: &BorrowedMessage<'_>) -> Self {
        let message_timestamp = match message.timestamp() {
            Timestamp::CreateTime(ms) | Timestamp::LogAppendTime(ms) => {
                DateTime::from_timestamp_millis(ms).unwrap_or_else(Utc::now)
            }
            Timestamp::NotAvailable => Utc::now(),
        };
        let envelope = Envelope::parse(message.headers(), message_timestamp);

        ConsumedRecord {
            topic: message.topic().to_string(),
            partition: message.partition(),
            offset: message.offset(),
            key: message.key().map(|k| String::from_utf8_lossy(k).into_owned()),
            payload: message.payload().map(|p| p.to_vec()).unwrap_or_default(),
            envelope,
        }
    }
}

/// Outcome of applying one batch — what the integration tests assert on.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BatchStats {
    pub applied: usize,
    pub skipped: usize,
}

enum ApplyError {
    Map(MapError),
    Db(DbErr),
}

impl From<DbErr> for ApplyError {
    fn from(e: DbErr) -> Self {
        ApplyError::Db(e)
    }
}

impl From<MapError> for ApplyError {
    fn from(e: MapError) -> Self {
        ApplyError::Map(e)
    }
}

/// Maps, upserts and commits offsets for one batch inside a single DB
/// transaction (§2.3). A mapping failure is a **poison record**: logged and
/// skipped, the rest of the batch still applies. A DB write failure aborts
/// the whole transaction — nothing in the batch commits, so the caller's
/// retry re-applies the identical batch next time, which every upsert here
/// is written to tolerate.
pub async fn process_batch(
    db: &DatabaseConnection,
    registry: &MapperRegistry,
    default_owner: Option<Uuid>,
    records: &[ConsumedRecord],
) -> Result<BatchStats, DbErr> {
    let txn = db.begin().await?;
    let mut stats = BatchStats::default();
    let mut next_offsets: HashMap<(String, i32), i64> = HashMap::new();

    for record in records {
        next_offsets.insert((record.topic.clone(), record.partition), record.offset + 1);

        let Some(kind) = entity_kind_for_topic(&record.topic) else {
            if let Some(labeling_kind) = labeling_topic_for(&record.topic) {
                match apply_labeling_record(&txn, labeling_kind, record).await {
                    Ok(()) => stats.applied += 1,
                    Err(e) => return Err(e),
                }
                continue;
            }
            warn!(topic = %record.topic, "projector: record on an unrecognized topic, skipped");
            stats.skipped += 1;
            continue;
        };

        let Some(mapper) = registry.resolve(&record.envelope.source, &record.envelope.origin) else {
            error!(
                topic = %record.topic,
                partition = record.partition,
                offset = record.offset,
                source = %record.envelope.source,
                origin = %record.envelope.origin,
                "projector: no mapper registered for this (source, origin); record skipped"
            );
            stats.skipped += 1;
            continue;
        };

        let input = MapperInput {
            event: SourceEvent {
                source: &record.envelope.source,
                source_account_id: record.envelope.source_account_id.as_deref(),
                key: record.key.as_deref().unwrap_or(""),
                payload: &record.payload,
                imported_at: record.envelope.imported_at,
            },
            comdirect_account_key: record.envelope.comdirect_account_key.as_deref(),
            comdirect_account_name: record.envelope.comdirect_account_name.as_deref(),
        };

        let outcome = match kind {
            EntityKind::Account => apply_account(&txn, mapper, &input, default_owner).await,
            EntityKind::Balance => apply_balance(&txn, mapper, &input, default_owner).await,
            EntityKind::Transaction => apply_transaction(&txn, mapper, &input, default_owner).await,
        };

        match outcome {
            Ok(()) => stats.applied += 1,
            Err(ApplyError::Map(map_err)) => {
                error!(
                    topic = %record.topic,
                    partition = record.partition,
                    offset = record.offset,
                    error = %map_err,
                    "projector: poison record skipped"
                );
                stats.skipped += 1;
            }
            Err(ApplyError::Db(db_err)) => return Err(db_err),
        }
    }

    for ((topic, partition), next_offset) in &next_offsets {
        offsets::commit_offset(&txn, topic, *partition, *next_offset, Utc::now()).await?;
    }

    txn.commit().await?;
    Ok(stats)
}

async fn apply_account(
    txn: &DatabaseTransaction,
    mapper: &dyn SourceMapper,
    input: &MapperInput<'_>,
    default_owner: Option<Uuid>,
) -> Result<(), ApplyError> {
    let record = mapper.map_account(input)?;
    // Checked before the upsert, which is the only way to tell "this record
    // created the row" from "this record updated an existing row" (§4) — the
    // upsert itself is one `INSERT ... ON CONFLICT DO UPDATE` statement with
    // no such signal of its own.
    let already_existed = upsert::account_exists(txn, record.id).await?;
    upsert::upsert_account(txn, &record).await?;
    if let Some(owner) = default_owner
        && !already_existed
    {
        upsert::link_default_owner(txn, owner, record.id, record.updated_at).await?;
    }
    Ok(())
}

async fn apply_balance(
    txn: &DatabaseTransaction,
    mapper: &dyn SourceMapper,
    input: &MapperInput<'_>,
    default_owner: Option<Uuid>,
) -> Result<(), ApplyError> {
    let record = mapper.map_balance(input)?;
    // A successful `map_balance` guarantees `source_account_id` was present
    // (§2.2) — that is exactly what it would have failed on otherwise.
    let source_account_id = input
        .event
        .source_account_id
        .expect("map_balance succeeded, so source_account_id must be present");

    let already_existed = upsert::account_exists(txn, record.account_id).await?;
    upsert::ensure_stub_account(
        txn,
        input.event.source,
        source_account_id,
        &record.currency,
        record.observed_at,
    )
    .await?;
    if let Some(owner) = default_owner
        && !already_existed
    {
        upsert::link_default_owner(txn, owner, record.account_id, record.observed_at).await?;
    }
    upsert::upsert_balance(txn, &record).await?;
    Ok(())
}

async fn apply_transaction(
    txn: &DatabaseTransaction,
    mapper: &dyn SourceMapper,
    input: &MapperInput<'_>,
    default_owner: Option<Uuid>,
) -> Result<(), ApplyError> {
    let record = mapper.map_transaction(input)?;
    let source_account_id = input
        .event
        .source_account_id
        .expect("map_transaction succeeded, so source_account_id must be present");

    let already_existed = upsert::account_exists(txn, record.account_id).await?;
    upsert::ensure_stub_account(
        txn,
        input.event.source,
        source_account_id,
        &record.currency,
        record.imported_at,
    )
    .await?;
    if let Some(owner) = default_owner
        && !already_existed
    {
        upsert::link_default_owner(txn, owner, record.account_id, record.imported_at).await?;
    }
    upsert::upsert_transaction(txn, &record).await?;
    Ok(())
}

/// Dispatches one record on a [`LabelingTopic`] to its
/// `projection::labeling::project_*` function, inside the same transaction
/// ingest records apply in. A malformed payload is a poison record — logged
/// and skipped, like [`ApplyError::Map`] — rather than aborting the batch; a
/// `DbErr` still aborts it, same as every other write here.
async fn apply_labeling_record(
    txn: &DatabaseTransaction,
    kind: LabelingTopic,
    record: &ConsumedRecord,
) -> Result<(), DbErr> {
    match kind {
        LabelingTopic::Category => {
            if record.payload.is_empty() {
                return match tombstone_uuid_from_key(record) {
                    Some(id) => labeling::project_category(txn, id, None).await,
                    None => Ok(()),
                };
            }
            match serde_json::from_slice::<CategoryRecord>(&record.payload) {
                Ok(parsed) => labeling::project_category(txn, parsed.id, Some(parsed)).await,
                Err(e) => {
                    error!(topic = %record.topic, offset = record.offset, error = %e, "projector: poison category record, skipped");
                    Ok(())
                }
            }
        }
        LabelingTopic::TransactionLabel => {
            if record.payload.is_empty() {
                return match tombstone_transaction_id(record) {
                    Some(id) => labeling::project_transaction_label(txn, id, None).await,
                    None => Ok(()),
                };
            }
            match serde_json::from_slice::<LabelRecord>(&record.payload) {
                Ok(parsed) => {
                    let id = transaction_uuid(&parsed.source, &parsed.external_id);
                    labeling::project_transaction_label(txn, id, Some(parsed)).await
                }
                Err(e) => {
                    error!(topic = %record.topic, offset = record.offset, error = %e, "projector: poison transaction-label record, skipped");
                    Ok(())
                }
            }
        }
        LabelingTopic::LlmCache => {
            let Some(fingerprint) = record.key.as_deref() else {
                warn!(topic = %record.topic, offset = record.offset, "projector: llm-cache record with no key, skipped");
                return Ok(());
            };
            if record.payload.is_empty() {
                return labeling::project_llm_cache(txn, fingerprint, None).await;
            }
            match serde_json::from_slice::<CacheRecord>(&record.payload) {
                Ok(parsed) => labeling::project_llm_cache(txn, fingerprint, Some(parsed)).await,
                Err(e) => {
                    error!(topic = %record.topic, offset = record.offset, error = %e, "projector: poison llm-cache record, skipped");
                    Ok(())
                }
            }
        }
        LabelingTopic::UserLabel => {
            if record.payload.is_empty() {
                return match tombstone_transaction_id(record) {
                    Some(id) => labeling::project_user_label(txn, id, None).await,
                    None => Ok(()),
                };
            }
            match serde_json::from_slice::<UserLabelRecord>(&record.payload) {
                Ok(parsed) => {
                    let id = transaction_uuid(&parsed.source, &parsed.external_id);
                    labeling::project_user_label(txn, id, Some(parsed)).await
                }
                Err(e) => {
                    error!(topic = %record.topic, offset = record.offset, error = %e, "projector: poison user-label record, skipped");
                    Ok(())
                }
            }
        }
        LabelingTopic::Rule => {
            let Some(id) = tombstone_uuid_from_key(record) else {
                warn!(topic = %record.topic, offset = record.offset, "projector: rule record with no/invalid-UUID key, skipped");
                return Ok(());
            };
            if record.payload.is_empty() {
                return labeling::project_rule(txn, id, None).await;
            }
            match serde_json::from_slice::<RuleRecord>(&record.payload) {
                Ok(parsed) => labeling::project_rule(txn, id, Some(parsed)).await,
                Err(e) => {
                    error!(topic = %record.topic, offset = record.offset, error = %e, "projector: poison rule record, skipped");
                    Ok(())
                }
            }
        }
    }
}

/// `category`/`rule` keys are the entity's own UUID verbatim.
fn tombstone_uuid_from_key(record: &ConsumedRecord) -> Option<Uuid> {
    Uuid::parse_str(record.key.as_deref()?).ok()
}

/// `transaction-label`/`user-label` keys are `<source>:<external_id>`
/// (§2.2) — the same form `labeling::processor::parse_source_external_id`
/// parses for the labeler's own tombstone handling.
fn tombstone_transaction_id(record: &ConsumedRecord) -> Option<Uuid> {
    let key = record.key.as_deref()?;
    let (source, external_id) = key.split_once(':')?;
    Some(transaction_uuid(source, external_id))
}

// ---------------------------------------------------------------------------
// The Kafka-facing half
// ---------------------------------------------------------------------------

/// Everything `run` needs besides the DB connection it is handed.
pub struct ProjectorConfig {
    pub brokers: String,
    pub batch_max_records: usize,
    pub batch_max_wait: Duration,
    pub max_consecutive_write_failures: u32,
    /// Resolved `user.id` for `APP_projector_default_owner` (§4), already
    /// looked up by the caller — `run` itself never touches usernames.
    pub default_owner: Option<Uuid>,
    /// `--until-caught-up`: exit once every ingest topic's high watermark has
    /// been reached, instead of polling forever (§2.3, handy for CI/tests).
    pub until_caught_up: bool,
}

impl ProjectorConfig {
    pub fn new(brokers: String) -> Self {
        Self {
            brokers,
            batch_max_records: DEFAULT_BATCH_MAX_RECORDS,
            batch_max_wait: DEFAULT_BATCH_MAX_WAIT,
            max_consecutive_write_failures: DEFAULT_MAX_CONSECUTIVE_WRITE_FAILURES,
            default_owner: None,
            until_caught_up: false,
        }
    }
}

#[derive(Debug)]
pub enum ProjectorError {
    Kafka(rdkafka::error::KafkaError),
    Db(DbErr),
    /// `max_consecutive_write_failures` batch writes in a row all failed;
    /// `run` stops instead of retrying forever against a DB that is not
    /// coming back on its own (§2.3).
    TooManyConsecutiveWriteFailures,
}

impl std::fmt::Display for ProjectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectorError::Kafka(e) => write!(f, "kafka error: {e}"),
            ProjectorError::Db(e) => write!(f, "database error: {e}"),
            ProjectorError::TooManyConsecutiveWriteFailures => {
                write!(f, "too many consecutive batch-write failures")
            }
        }
    }
}

impl std::error::Error for ProjectorError {}

/// Consumes the ingest topics and projects every record into the read model,
/// forever (or until caught up, with `--until-caught-up`) — (§2.3).
///
/// No consumer group: offsets live in Postgres (`offsets::load_offsets`,
/// committed alongside each batch's rows), and `assign()` resumes from
/// exactly there, defaulting to `Offset::Beginning` for a topic this
/// projector has never seen.
pub async fn run(db: DatabaseConnection, config: ProjectorConfig) -> Result<(), ProjectorError> {
    let registry = MapperRegistry::with_default_mappers();

    let consumer: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", &config.brokers)
        // No consumer-group coordination is used (offsets live in Postgres,
        // not Kafka, and partitions are always `assign()`ed explicitly) --
        // but librdkafka still requires `group.id` to be a non-empty string
        // on every consumer, group membership or not.
        .set("group.id", "finreport-projector")
        .set("enable.auto.commit", "false")
        .set("enable.partition.eof", "false")
        .create()
        .map_err(ProjectorError::Kafka)?;

    let stored_offsets = offsets::load_offsets(&db).await.map_err(ProjectorError::Db)?;
    let mut tpl = TopicPartitionList::new();
    for topic in INGEST_TOPICS.into_iter().chain(LABELING_PROJECTION_TOPICS) {
        let offset = stored_offsets
            .get(&(topic.to_string(), INGEST_PARTITION))
            .map(|&next| Offset::Offset(next))
            .unwrap_or(Offset::Beginning);
        tpl.add_partition_offset(topic, INGEST_PARTITION, offset)
            .map_err(ProjectorError::Kafka)?;
    }
    consumer.assign(&tpl).map_err(ProjectorError::Kafka)?;

    // `--until-caught-up` needs each topic's high watermark *and* where this
    // run is actually starting from, tracked ourselves as `next_offsets`
    // rather than read back via `consumer.position()`: librdkafka only
    // reports a partition's position once a fetch response for it has
    // actually landed, so right after `assign()` -- the common case for a
    // topic that is already fully caught up and will never receive one --
    // `position()` reports `Offset::Invalid` forever, and a loop driven by
    // it never terminates. `next_offsets` starts at the real resume point
    // (the stored offset, or the topic's low watermark for a from-scratch
    // `Offset::Beginning`) and advances by hand as records are consumed.
    let mut next_offsets: HashMap<String, i64> = HashMap::new();
    let high_watermarks = if config.until_caught_up {
        let mut marks = HashMap::new();
        for topic in INGEST_TOPICS.into_iter().chain(LABELING_PROJECTION_TOPICS) {
            let (low, high) = consumer
                .fetch_watermarks(topic, INGEST_PARTITION, Duration::from_secs(10))
                .map_err(ProjectorError::Kafka)?;
            marks.insert(topic.to_string(), high);
            let starting = stored_offsets
                .get(&(topic.to_string(), INGEST_PARTITION))
                .copied()
                .unwrap_or(low);
            next_offsets.insert(topic.to_string(), starting);
        }
        Some(marks)
    } else {
        None
    };

    let mut consecutive_write_failures = 0u32;

    loop {
        let batch = collect_batch(&consumer, config.batch_max_records, config.batch_max_wait).await;

        if let Some(last_by_topic) = last_offset_per_topic(&batch) {
            for (topic, last_offset) in last_by_topic {
                next_offsets.insert(topic, last_offset + 1);
            }
        }

        if batch.is_empty() {
            if let Some(marks) = &high_watermarks {
                if is_caught_up(&next_offsets, marks) {
                    info!("projector: caught up with all ingest topics, exiting (--until-caught-up)");
                    return Ok(());
                }
            }
            continue;
        }

        let (stats, failures) = retry_batch_write(
            || process_batch(&db, &registry, config.default_owner, &batch),
            consecutive_write_failures,
            config.max_consecutive_write_failures,
        )
        .await?;
        consecutive_write_failures = failures;
        info!(applied = stats.applied, skipped = stats.skipped, "projector: batch applied");
    }
}

/// Calls `write` (a closure that closes over one fixed batch, e.g.
/// `|| process_batch(&db, &registry, owner, &batch)`), retrying the
/// **identical** batch with backoff on a DB failure instead of letting the
/// caller move on to a fresh one.
///
/// This is the fix for the data-loss bug `run`'s loop used to have: on a
/// failed write it previously went straight back to `collect_batch`, which
/// pulls *new* messages from the live `StreamConsumer` — the failed batch's
/// records were never retried, never committed, and (since the consumer's
/// read position had already moved past them) unrecoverable even on restart
/// within the same run, because `run`'s own `next_offsets`/the committed
/// Postgres offsets never covered them either. Retrying the same `batch`
/// value here means nothing is ever skipped: either it eventually succeeds
/// (and its offsets commit, in the same transaction, exactly once) or the
/// failure streak reaches `max_failures` and the process exits non-zero
/// before ever asking the consumer for more messages.
///
/// `consecutive_failures` carries the streak across calls (batches) so the
/// limit is on the number of consecutive failures across the whole run, not
/// reset to zero by each new batch.
async fn retry_batch_write<F, Fut>(
    write: F,
    mut consecutive_failures: u32,
    max_failures: u32,
) -> Result<(BatchStats, u32), ProjectorError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<BatchStats, DbErr>>,
{
    loop {
        match write().await {
            Ok(stats) => return Ok((stats, 0)),
            Err(db_err) => {
                consecutive_failures += 1;
                error!(
                    error = %db_err,
                    attempt = consecutive_failures,
                    "projector: batch write failed, retrying the same batch"
                );
                if consecutive_failures >= max_failures {
                    return Err(ProjectorError::TooManyConsecutiveWriteFailures);
                }
                let backoff_ms = 200u64.saturating_mul(1u64 << consecutive_failures.min(5));
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }
        }
    }
}

async fn collect_batch(
    consumer: &StreamConsumer,
    max_records: usize,
    max_wait: Duration,
) -> Vec<ConsumedRecord> {
    let mut batch = Vec::new();
    let deadline = tokio::time::Instant::now() + max_wait;

    loop {
        if batch.len() >= max_records {
            break;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        match tokio::time::timeout(remaining, consumer.recv()).await {
            Ok(Ok(message)) => batch.push(ConsumedRecord::from_message(&message)),
            Ok(Err(e)) => warn!(error = %e, "projector: kafka poll error"),
            Err(_elapsed) => break,
        }
    }

    batch
}

/// The offset after the last record this batch carried, per topic — what
/// `next_offsets` advances to once a batch (even a to-be-discarded empty
/// one) has been collected.
fn last_offset_per_topic(batch: &[ConsumedRecord]) -> Option<HashMap<String, i64>> {
    if batch.is_empty() {
        return None;
    }
    let mut last: HashMap<String, i64> = HashMap::new();
    for record in batch {
        last.entry(record.topic.clone())
            .and_modify(|o| *o = (*o).max(record.offset))
            .or_insert(record.offset);
    }
    Some(last)
}

/// Whether `next_offsets` (this run's own resume-point bookkeeping, §2.3 --
/// *not* `consumer.position()`, see `run`'s comment on why) has reached
/// every ingest topic's high watermark as of `assign()` time
/// (`--until-caught-up`). An empty topic (`high == 0`) is trivially caught up
/// regardless of position.
fn is_caught_up(next_offsets: &HashMap<String, i64>, high_watermarks: &HashMap<String, i64>) -> bool {
    for topic in INGEST_TOPICS.into_iter().chain(LABELING_PROJECTION_TOPICS) {
        let high = *high_watermarks.get(topic).unwrap_or(&0);
        if high == 0 {
            continue;
        }
        let current = next_offsets.get(topic).copied().unwrap_or(0);
        if current < high {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod retry_tests {
    //! Unit tests for `retry_batch_write` (§2.3 fix): a transient write
    //! failure must retry the *same* batch, not silently move on to a new
    //! one, and the retry must stop (process exits non-zero via
    //! `TooManyConsecutiveWriteFailures`) once the failure streak reaches
    //! the configured limit. No DB or Kafka involved — `write` here is a
    //! plain injectable closure standing in for `process_batch`.

    use super::*;
    use std::cell::RefCell;

    fn fake_record(offset: i64) -> ConsumedRecord {
        ConsumedRecord {
            topic: TOPIC_ACCOUNT.to_string(),
            partition: 0,
            offset,
            key: Some("k".to_string()),
            payload: Vec::new(),
            envelope: Envelope {
                source: "comdirect".to_string(),
                source_account_id: None,
                origin: "source".to_string(),
                schema_version: 1,
                imported_at: Utc::now(),
                comdirect_account_key: None,
                comdirect_account_name: None,
            },
        }
    }

    /// A DB write that fails `fail_times` times in a row, then succeeds,
    /// recording the batch (by its offsets) it was handed on every attempt
    /// — what proves the retry re-sends the identical batch rather than a
    /// fresh one.
    struct FlakyWriter {
        fail_times: u32,
        calls: RefCell<Vec<Vec<i64>>>,
    }

    impl FlakyWriter {
        fn new(fail_times: u32) -> Self {
            Self { fail_times, calls: RefCell::new(Vec::new()) }
        }

        fn call(&self, batch: &[ConsumedRecord]) -> Result<BatchStats, DbErr> {
            let attempt = self.calls.borrow().len() as u32;
            self.calls.borrow_mut().push(batch.iter().map(|r| r.offset).collect());
            if attempt < self.fail_times {
                Err(DbErr::Custom("simulated transient failure".to_string()))
            } else {
                Ok(BatchStats { applied: batch.len(), skipped: 0 })
            }
        }
    }

    #[tokio::test]
    async fn transient_failure_retries_the_same_batch_until_it_succeeds() {
        let batch = vec![fake_record(10), fake_record(11), fake_record(12)];
        let writer = FlakyWriter::new(2);

        let (stats, failures) =
            retry_batch_write(|| std::future::ready(writer.call(&batch)), 0, 5).await.unwrap();

        assert_eq!(stats.applied, 3, "the batch must eventually be written in full");
        assert_eq!(failures, 0, "a successful write resets the failure streak");

        let calls = writer.calls.borrow();
        assert_eq!(calls.len(), 3, "two failures then one success = three attempts");
        for call in calls.iter() {
            assert_eq!(
                call,
                &vec![10, 11, 12],
                "every retry must carry the identical batch (same offsets) — \
                 none of the original records may be dropped or swapped for a new poll"
            );
        }
    }

    #[tokio::test]
    async fn persistent_failure_stops_after_the_configured_limit_without_skipping_offsets() {
        let batch = vec![fake_record(0)];
        // Every call fails -- more than `max_failures` worth.
        let writer = FlakyWriter::new(u32::MAX);

        let result = retry_batch_write(|| std::future::ready(writer.call(&batch)), 0, 3).await;

        assert!(
            matches!(result, Err(ProjectorError::TooManyConsecutiveWriteFailures)),
            "a persistently failing batch must give up, not silently advance past it"
        );
        assert_eq!(
            writer.calls.borrow().len(),
            3,
            "exactly `max_failures` attempts, all on the same unwritten batch"
        );
    }

    #[tokio::test]
    async fn failure_streak_carries_across_batches() {
        // Simulates `run`'s loop calling `retry_batch_write` once per batch:
        // a streak started on an earlier batch must still count toward the
        // limit on a later one, not reset just because the batch changed.
        let batch = vec![fake_record(0)];
        let writer = FlakyWriter::new(u32::MAX);

        let result = retry_batch_write(|| std::future::ready(writer.call(&batch)), 2, 3).await;

        assert!(matches!(result, Err(ProjectorError::TooManyConsecutiveWriteFailures)));
        assert_eq!(
            writer.calls.borrow().len(),
            1,
            "starting already at 2 failures, the very next failure (3) must hit the limit"
        );
    }
}
