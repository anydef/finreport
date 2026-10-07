//! Header repair for records published without `source_account_id`.
//!
//! A historical producer omitted that header; the projector's mappers reject
//! such records as poison and drop them. The fix is to republish each one on
//! the same topic and key with a complete header set and the **original value
//! bytes untouched** (the value is the bank's own JSON). Topics are keyed, so
//! the republished record supersedes the broken one.
//!
//! Everything here is pure (no Kafka, no Postgres): the binary feeds it
//! scanned records plus a `reference -> account_id` lookup and publishes the
//! resulting [`Repair`]s.

use std::collections::HashMap;

use chrono::Utc;
use rdkafka::message::OwnedHeaders;
use tracing::{debug, warn};

use super::envelope::{
    CURRENT_SCHEMA_VERSION, HEADER_COMDIRECT_ACCOUNT_KEY, HEADER_COMDIRECT_ACCOUNT_NAME,
    HEADER_IMPORTED_AT, HEADER_SOURCE_ACCOUNT_ID, ORIGIN_SOURCE, RecordMeta, SOURCE_COMDIRECT,
    TOPIC_ACCOUNT, TOPIC_ACCOUNT_BALANCE, TOPIC_TRANSACTION,
};
use super::producer::EventPublisher;
use super::scan::{ScannedRecord, scan_topic};

/// Topics the repair scans, in processing order.
pub const REPAIR_TOPICS: [&str; 3] = [TOPIC_TRANSACTION, TOPIC_ACCOUNT_BALANCE, TOPIC_ACCOUNT];

/// `comdirect_account_key` when the broken record carried none.
const UNKNOWN_ACCOUNT_KEY: &str = "unknown";

/// Whether a live record lacks a usable `source_account_id`. Tombstones are
/// never repaired (there is nothing to project).
pub fn needs_repair(record: &ScannedRecord) -> bool {
    record.payload.is_some()
        && record
            .header(HEADER_SOURCE_ACCOUNT_ID)
            .is_none_or(|v| v.trim().is_empty())
}

/// Owned counterpart of [`RecordMeta`] for a repaired record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairedMeta {
    pub source_account_id: String,
    pub imported_at: String,
    pub comdirect_account_key: String,
    pub comdirect_account_name: Option<String>,
    /// True when the broken record had no `imported_at` and "now" was used.
    pub imported_at_defaulted: bool,
}

impl RepairedMeta {
    pub fn as_record_meta(&self) -> RecordMeta<'_> {
        RecordMeta {
            source: SOURCE_COMDIRECT,
            source_account_id: Some(&self.source_account_id),
            origin: ORIGIN_SOURCE,
            schema_version: CURRENT_SCHEMA_VERSION,
            imported_at: &self.imported_at,
            comdirect_account_key: &self.comdirect_account_key,
            comdirect_account_name: self.comdirect_account_name.as_deref(),
        }
    }

    pub fn headers(&self) -> OwnedHeaders {
        self.as_record_meta().headers()
    }
}

/// Builds the corrected header set from a broken record's own headers:
/// `imported_at`, account key and account name are carried over when present.
pub fn repaired_meta(broken: &ScannedRecord, account_id: &str) -> RepairedMeta {
    let carried_imported_at = broken.header(HEADER_IMPORTED_AT).filter(|v| !v.is_empty());
    RepairedMeta {
        source_account_id: account_id.to_string(),
        imported_at: carried_imported_at
            .map(str::to_string)
            .unwrap_or_else(|| Utc::now().to_rfc3339()),
        comdirect_account_key: broken
            .header(HEADER_COMDIRECT_ACCOUNT_KEY)
            .unwrap_or(UNKNOWN_ACCOUNT_KEY)
            .to_string(),
        comdirect_account_name: broken
            .header(HEADER_COMDIRECT_ACCOUNT_NAME)
            .map(str::to_string),
        imported_at_defaulted: carried_imported_at.is_none(),
    }
}

/// One record to republish: same topic, same key, original bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair {
    pub topic: &'static str,
    pub key: String,
    pub payload: Vec<u8>,
    pub meta: RepairedMeta,
}

/// Outcome of planning the repair of one topic.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RepairPlan {
    pub repairs: Vec<Repair>,
    /// Keys of broken records whose account id could not be determined
    /// (`None` entries are keyless records).
    pub unrecoverable: Vec<Option<String>>,
}

/// Which records of `topic` matter. Account and transaction topics are
/// compacted: only the newest record per key survives, so an older broken
/// record that a later one already superseded must NOT be republished (it
/// would regress the key). The balance topic is an uncompacted observation
/// log: the broken originals stay on it forever after a repair, so a broken
/// record only counts while no repaired copy of it exists (see
/// [`already_repaired`]) -- that is what keeps a re-run at zero. Tombstones
/// are dropped here (a tombstoned key is gone).
fn effective_records<'a>(topic: &str, records: &'a [ScannedRecord]) -> Vec<&'a ScannedRecord> {
    if topic == TOPIC_ACCOUNT_BALANCE {
        return records
            .iter()
            .filter(|r| !needs_repair(r) || !already_repaired(r, records))
            .collect();
    }
    let mut latest: HashMap<&str, usize> = HashMap::new();
    let mut keyless = Vec::new();
    for (idx, record) in records.iter().enumerate() {
        match record.key.as_deref() {
            Some(key) => {
                latest.insert(key, idx);
            }
            None => keyless.push(idx),
        }
    }
    let mut indices: Vec<usize> = latest.into_values().chain(keyless).collect();
    indices.sort_unstable();
    indices.into_iter().map(|i| &records[i]).collect()
}

/// Whether `broken` already has a repaired copy in `records`: same key, same
/// value bytes, a `source_account_id`, and the same `imported_at` (repair
/// carries it over; a broken record with none can never match on it, so its
/// key and bytes alone identify the copy).
fn already_repaired(broken: &ScannedRecord, records: &[ScannedRecord]) -> bool {
    records.iter().any(|candidate| {
        !needs_repair(candidate)
            && candidate.payload.is_some()
            && candidate.key == broken.key
            && candidate.payload == broken.payload
            && broken
                .header(HEADER_IMPORTED_AT)
                .is_none_or(|at| candidate.header(HEADER_IMPORTED_AT) == Some(at))
    })
}

/// Plans the repairs for `topic`. For account and balance topics the record
/// key is the account id; for transactions it is a `reference`, resolved
/// through `account_by_reference` (never guessed: a miss is unrecoverable).
pub fn plan_topic_repairs(
    topic: &'static str,
    records: &[ScannedRecord],
    account_by_reference: &HashMap<String, String>,
) -> RepairPlan {
    let mut plan = RepairPlan::default();
    for record in effective_records(topic, records)
        .into_iter()
        .filter(|r| needs_repair(r))
    {
        let account_id = match (topic, record.key.as_deref()) {
            (_, None) => None,
            (TOPIC_TRANSACTION, Some(reference)) => account_by_reference.get(reference).cloned(),
            (_, Some(key)) => Some(key.to_string()),
        };
        match (account_id, record.key.as_ref(), record.payload.as_ref()) {
            (Some(account_id), Some(key), Some(payload)) => plan.repairs.push(Repair {
                topic,
                key: key.clone(),
                payload: payload.clone(),
                meta: repaired_meta(record, &account_id),
            }),
            _ => plan.unrecoverable.push(record.key.clone()),
        }
    }
    plan
}

/// What a repair run did (or, for a dry run, would do).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RepairReport {
    /// Repaired record count per topic, in [`REPAIR_TOPICS`] order.
    pub repaired: Vec<(&'static str, usize)>,
    pub unrecoverable: usize,
    pub imported_at_defaulted: usize,
}

/// Scans [`REPAIR_TOPICS`] and republishes every repairable record through
/// `publisher`. With `publisher: None` nothing is published (dry run) but the
/// report is identical to what a real run would produce.
pub async fn repair_headers(
    brokers: &str,
    publisher: Option<&EventPublisher>,
    account_by_reference: &HashMap<String, String>,
) -> Result<RepairReport, Box<dyn std::error::Error>> {
    let mut report = RepairReport::default();
    for topic in REPAIR_TOPICS {
        let plan = plan_topic_repairs(topic, &scan_topic(brokers, topic)?, account_by_reference);
        for key in &plan.unrecoverable {
            warn!(%topic, key = key.as_deref().unwrap_or("<none>"), "[repair-headers] account id unrecoverable; skipping");
        }
        for repair in &plan.repairs {
            if repair.meta.imported_at_defaulted {
                warn!(%topic, key = %repair.key, "[repair-headers] record has no imported_at; using the current time");
                report.imported_at_defaulted += 1;
            }
            if let Some(publisher) = publisher {
                publisher
                    .publish_with_headers(
                        repair.topic,
                        &repair.key,
                        &repair.payload,
                        repair.meta.headers(),
                    )
                    .await?;
            }
            debug!(%topic, key = %repair.key, "[repair-headers] republished");
        }
        report.unrecoverable += plan.unrecoverable.len();
        report.repaired.push((topic, plan.repairs.len()));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(key: &str, payload: Option<&str>, headers: &[(&str, &str)]) -> ScannedRecord {
        ScannedRecord {
            key: Some(key.to_string()),
            payload: payload.map(|p| p.as_bytes().to_vec()),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn lookup(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn record_missing_the_header_is_selected() {
        assert!(needs_repair(&rec(
            "k",
            Some("{}"),
            &[("comdirect_account_key", "0")]
        )));
    }

    #[test]
    fn record_with_the_header_is_not_selected() {
        assert!(!needs_repair(&rec(
            "k",
            Some("{}"),
            &[("source_account_id", "A")]
        )));
    }

    #[test]
    fn tombstones_are_not_selected() {
        assert!(!needs_repair(&rec("k", None, &[])));
    }

    #[test]
    fn headers_carry_over_and_fill_in_the_rest() {
        let broken = rec(
            "ref/1",
            Some("{}"),
            &[
                ("comdirect_account_key", "0"),
                ("imported_at", "2026-09-28T20:17:17.460978387+00:00"),
                ("comdirect_account_name", "Comdirect Family"),
            ],
        );
        let meta = repaired_meta(&broken, "ACC");
        assert_eq!(meta.source_account_id, "ACC");
        assert_eq!(meta.imported_at, "2026-09-28T20:17:17.460978387+00:00");
        assert!(!meta.imported_at_defaulted);
        assert_eq!(meta.comdirect_account_key, "0");
        assert_eq!(
            meta.comdirect_account_name.as_deref(),
            Some("Comdirect Family")
        );
        let record_meta = meta.as_record_meta();
        assert_eq!(record_meta.origin, ORIGIN_SOURCE);
        assert_eq!(record_meta.source, SOURCE_COMDIRECT);
        assert_eq!(record_meta.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn missing_imported_at_defaults_to_now_and_is_flagged() {
        let meta = repaired_meta(&rec("k", Some("{}"), &[]), "ACC");
        assert!(meta.imported_at_defaulted);
        assert!(chrono::DateTime::parse_from_rfc3339(&meta.imported_at).is_ok());
        assert_eq!(meta.comdirect_account_key, UNKNOWN_ACCOUNT_KEY);
        assert_eq!(meta.comdirect_account_name, None);
    }

    #[test]
    fn account_and_balance_topics_use_the_key_as_account_id() {
        for topic in [TOPIC_ACCOUNT, TOPIC_ACCOUNT_BALANCE] {
            let plan = plan_topic_repairs(topic, &[rec("ACC-1", Some("{}"), &[])], &HashMap::new());
            assert_eq!(plan.repairs.len(), 1);
            assert_eq!(plan.repairs[0].meta.source_account_id, "ACC-1");
            assert_eq!(plan.repairs[0].key, "ACC-1");
            assert_eq!(plan.repairs[0].topic, topic);
            assert!(plan.unrecoverable.is_empty());
        }
    }

    #[test]
    fn transactions_resolve_the_account_through_the_legacy_lookup_and_keep_bytes() {
        let raw = "{ \"reference\" :  \"R/1\" }";
        let plan = plan_topic_repairs(
            TOPIC_TRANSACTION,
            &[rec("R/1", Some(raw), &[])],
            &lookup(&[("R/1", "ACC-9")]),
        );
        assert_eq!(plan.repairs[0].meta.source_account_id, "ACC-9");
        assert_eq!(plan.repairs[0].payload, raw.as_bytes());
    }

    #[test]
    fn unknown_transaction_reference_is_counted_and_skipped_not_guessed() {
        let plan = plan_topic_repairs(
            TOPIC_TRANSACTION,
            &[
                rec("R/known", Some("{}"), &[]),
                rec("R/unknown", Some("{}"), &[("comdirect_account_key", "0")]),
            ],
            &lookup(&[("R/known", "ACC-1")]),
        );
        assert_eq!(plan.repairs.len(), 1);
        assert_eq!(plan.unrecoverable, vec![Some("R/unknown".to_string())]);
    }

    #[test]
    fn compacted_topic_ignores_a_broken_record_superseded_by_a_good_one() {
        let records = [
            rec("ACC-1", Some("{}"), &[]),
            rec("ACC-1", Some("{}"), &[("source_account_id", "ACC-1")]),
        ];
        let plan = plan_topic_repairs(TOPIC_ACCOUNT, &records, &HashMap::new());
        assert!(plan.repairs.is_empty());
    }

    #[test]
    fn balance_topic_repairs_every_broken_observation() {
        let records = [
            rec(
                "ACC-1",
                Some("{}"),
                &[("imported_at", "2026-01-01T00:00:00Z")],
            ),
            rec(
                "ACC-1",
                Some("{}"),
                &[("imported_at", "2026-01-02T00:00:00Z")],
            ),
        ];
        let plan = plan_topic_repairs(TOPIC_ACCOUNT_BALANCE, &records, &HashMap::new());
        assert_eq!(plan.repairs.len(), 2);
    }

    #[test]
    fn balance_observation_with_a_repaired_copy_is_not_repaired_again() {
        let broken = rec(
            "ACC-1",
            Some("{}"),
            &[("imported_at", "2026-01-01T00:00:00Z")],
        );
        let copy = rec(
            "ACC-1",
            Some("{}"),
            &[
                ("imported_at", "2026-01-01T00:00:00Z"),
                ("source_account_id", "ACC-1"),
            ],
        );
        let other_day = rec(
            "ACC-1",
            Some("{}"),
            &[("imported_at", "2026-01-02T00:00:00Z")],
        );
        let plan = plan_topic_repairs(
            TOPIC_ACCOUNT_BALANCE,
            &[broken, copy, other_day],
            &HashMap::new(),
        );
        assert_eq!(
            plan.repairs.len(),
            1,
            "only the un-copied observation remains"
        );
        assert_eq!(plan.repairs[0].meta.imported_at, "2026-01-02T00:00:00Z");
    }

    #[test]
    fn repaired_records_are_not_selected_again() {
        let plan = plan_topic_repairs(
            TOPIC_ACCOUNT,
            &[rec("ACC-1", Some("{}"), &[])],
            &HashMap::new(),
        );
        let repaired = &plan.repairs[0];
        let headers = repaired.meta.as_record_meta();
        let republished = ScannedRecord {
            key: Some(repaired.key.clone()),
            payload: Some(repaired.payload.clone()),
            headers: vec![(
                HEADER_SOURCE_ACCOUNT_ID.to_string(),
                headers.source_account_id.unwrap().to_string(),
            )],
        };
        assert!(!needs_repair(&republished));
    }
}
