//! `uploadC24Transactions`: import C24 CSV exports.
//!
//! The write path in front of `finreport.c24-account` / `finreport.c24-transaction`
//! and `projection::c24::C24Mapper`. Same shape as `uploadPurchaseOrders`:
//! parse every file, merge, publish only what is new or changed, project what
//! reached the broker even if a later publish failed, and report per file.
//!
//! **Not admin-gated, on purpose.** A user importing their own bank statement
//! is their own data (the boundary settled 2026-10-25), like a goal or a
//! display alias. The `require_admin` resolvers in `current_user.rs` are
//! taxonomy- and rule-level. Do not "fix" this.
//!
//! **Why this does not use `events::publish_event`.** That helper builds a
//! two-header set and nothing else. `C24Mapper::map_transaction` reads
//! `source_account_id` off the envelope and fails without it, so every record
//! would skip silently on replay. Records go through [`RecordMeta`] and the
//! ack-awaiting [`EventPublisher::publish`]: a lost transaction is data loss.
//!
//! **Why this does not use `process_batch`.** That commits projector offsets
//! under a consumer group; this request is not a consumer. It calls the same
//! `apply_account` / `apply_transaction` the projector does, on the exact bytes
//! it published, and the real projector later re-applies them idempotently.
//!
//! **Visibility.** `AuthenticatedUser::account_ids` is what scopes every
//! query, so each imported account is linked to the caller in `user_account`
//! (idempotently, on every upload). Without it the import would succeed and
//! show nothing.
//!
//! **Cross-user safety.** Account ids are derived from the account number
//! alone, so two users uploading the same number would address one account
//! row. If an account is already linked to *other* users and not to the
//! caller, the upload is refused: otherwise anyone who knew a number could
//! link themselves to someone else's transactions.
//!
//! **Privacy.** Every cell is bank data. Parser errors are value-free by
//! construction, and nothing here logs or returns file content, a row, or an
//! account number.

use async_graphql::{ErrorExtensions, SimpleObject};
use c24_csv::C24Row;
use chrono::{DateTime, SecondsFormat, Utc};
use entity::entities::{account, transaction, user_account};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect,
    TransactionTrait,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

use crate::graphql::current_user::AuthenticatedUser;
use crate::graphql::events::kafka_unavailable_error;
use crate::graphql::scalars::Date as GqlDate;
use crate::kafka::c24::{
    C24AccountRecord, C24TransactionRecord, CURRENT_SCHEMA_VERSION, SOURCE_C24, TOPIC_C24_ACCOUNT,
    TOPIC_C24_TRANSACTION,
};
use crate::kafka::envelope::{
    account_uuid, transaction_uuid, RecordMeta, SourceContext, SourceEvent, ORIGIN_SOURCE,
};
use crate::kafka::producer::EventPublisher;
use crate::projection::c24::C24Mapper;
use crate::projection::mapper::MapperInput;
use crate::projection::upsert::link_default_owner;
use crate::projection::{apply_account, apply_transaction, ApplyError};

/// Files per upload, bytes per file and rows per upload. A real export is far
/// below these; they only stop a runaway request.
pub const MAX_FILES: usize = 20;
pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_ROWS: usize = 50_000;
const ID_CHUNK: usize = 5_000;

#[derive(SimpleObject, Clone, Debug, PartialEq)]
pub struct C24FileResult {
    /// Position in the `files` argument, from 0.
    pub index: i32,
    /// Rows read from the file; 0 when it was rejected.
    pub row_count: i32,
    /// Why the file was rejected: column names and line numbers, never a
    /// cell. `null` when the file was read.
    pub error: Option<String>,
}

#[derive(SimpleObject, Clone, Debug, PartialEq, Default)]
pub struct C24UploadResult {
    /// One entry per uploaded file, in order.
    pub files: Vec<C24FileResult>,
    /// Transactions not seen before.
    pub new_transactions: i32,
    /// Transactions already stored exactly as uploaded; nothing was published
    /// for them. An overlapping export lands mostly here.
    pub unchanged_transactions: i32,
    /// Transactions seen before whose content changed (C24 re-categorised a
    /// row, say). The identity hash ignores those fields, so the id is kept.
    pub updated_transactions: i32,
    /// Accounts not seen before.
    pub new_accounts: i32,
    /// Accounts seen before whose record changed (a renamed account).
    pub updated_accounts: i32,
    /// Distinct accounts across the readable files.
    pub account_count: i32,
    /// Earliest booking date across the readable files; `null` when no file
    /// was readable or all were empty.
    pub earliest_booking_date: Option<GqlDate>,
    /// Latest booking date across the readable files.
    pub latest_booking_date: Option<GqlDate>,
}

fn validation_error(message: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(message.into()).extend_with(|_, e| e.set("code", "VALIDATION"))
}

fn publish_failed(topic: &str, e: rdkafka::error::KafkaError) -> async_graphql::Error {
    // `KafkaError`'s text names the broker condition, never the payload.
    async_graphql::Error::new(format!("failed to publish to {topic}: {e}"))
        .extend_with(|_, ext| ext.set("code", "KAFKA_PUBLISH_FAILED"))
}

fn serialize_failed() -> async_graphql::Error {
    async_graphql::Error::new("failed to serialize a C24 record")
}

/// Parses each file, isolating failures per file, and merges the good ones.
/// Pure, so the limits and the isolation are unit-tested without a database.
pub fn parse_files(files: &[String]) -> Result<(Vec<C24FileResult>, Vec<C24Row>), String> {
    if files.is_empty() {
        return Err("no files given".into());
    }
    if files.len() > MAX_FILES {
        return Err(format!("at most {MAX_FILES} files per upload"));
    }
    let mut results = Vec::new();
    let mut parsed = Vec::new();
    for (index, content) in files.iter().enumerate() {
        let outcome = if content.len() > MAX_FILE_BYTES {
            Err(format!("file is larger than {} MB", MAX_FILE_BYTES / (1024 * 1024)))
        } else {
            // `ParseError` is value-free by construction.
            c24_csv::parse(content.as_bytes()).map_err(|e| e.to_string())
        };
        match outcome {
            Ok(rows) => {
                results.push(C24FileResult { index: index as i32, row_count: rows.len() as i32, error: None });
                parsed.push(rows);
            }
            Err(error) => {
                results.push(C24FileResult { index: index as i32, row_count: 0, error: Some(error) })
            }
        }
    }
    let merged = c24_csv::merge(parsed);
    if merged.len() > MAX_ROWS {
        return Err(format!("at most {MAX_ROWS} rows per upload"));
    }
    Ok((results, merged))
}

/// What an upload does to one stored entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    New,
    Updated,
    Unchanged,
}

/// Compares the record about to be published with the stored row's
/// `raw_payload` (the mapper keeps the published payload there verbatim).
pub fn classify(stored: Option<&Value>, new: &Value) -> Change {
    match stored {
        None => Change::New,
        Some(s) if s == new => Change::Unchanged,
        Some(_) => Change::Updated,
    }
}

/// One record per distinct account number, the first row's name winning.
pub fn account_records(rows: &[C24Row]) -> Vec<C24AccountRecord> {
    let mut seen = HashSet::new();
    rows.iter()
        .filter(|r| seen.insert(r.account_number.clone()))
        .map(C24AccountRecord::from_row)
        .collect()
}

/// Refuses accounts that belong to other users and not to the caller.
async fn check_ownership(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    account_ids: &[Uuid],
) -> async_graphql::Result<()> {
    if account_ids.is_empty() {
        return Ok(());
    }
    let links = user_account::Entity::find()
        .filter(user_account::Column::AccountId.is_in(account_ids.to_vec()))
        .all(db)
        .await?;
    let mine: HashSet<Uuid> = links.iter().filter(|l| l.user_id == user_id).map(|l| l.account_id).collect();
    if links.iter().any(|l| l.user_id != user_id && !mine.contains(&l.account_id)) {
        return Err(validation_error(
            "an account in this upload already belongs to another user; nothing was imported",
        ));
    }
    Ok(())
}

/// `(origin, raw_payload)` of the stored accounts among `ids`.
async fn stored_accounts(
    db: &impl ConnectionTrait,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, (String, Option<Value>)>, sea_orm::DbErr> {
    Ok(account::Entity::find()
        .filter(account::Column::Id.is_in(ids.to_vec()))
        .all(db)
        .await?
        .into_iter()
        .map(|a| (a.id, (a.origin, a.raw_payload)))
        .collect())
}

async fn stored_transaction_payloads(
    db: &impl ConnectionTrait,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Value>, sea_orm::DbErr> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(ID_CHUNK) {
        let rows: Vec<(Uuid, Value)> = transaction::Entity::find()
            .select_only()
            .column(transaction::Column::Id)
            .column(transaction::Column::RawPayload)
            .filter(transaction::Column::Id.is_in(chunk.to_vec()))
            .into_tuple()
            .all(db)
            .await?;
        out.extend(rows);
    }
    Ok(out)
}

/// A record that reached the broker, with the bytes that did.
struct Published {
    topic: &'static str,
    key: String,
    payload: Vec<u8>,
    source_account_id: Option<String>,
}

pub async fn upload_c24_transactions(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    files: Vec<String>,
) -> async_graphql::Result<C24UploadResult> {
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let (file_results, rows) = parse_files(&files).map_err(validation_error)?;
    let mut result = C24UploadResult { files: file_results, ..Default::default() };

    let accounts = account_records(&rows);
    result.account_count = accounts.len() as i32;
    result.earliest_booking_date = rows.iter().map(|r| r.booking_date).min().map(GqlDate);
    result.latest_booking_date = rows.iter().map(|r| r.booking_date).max().map(GqlDate);

    let account_ids: Vec<Uuid> = accounts.iter().map(|a| account_uuid(SOURCE_C24, &a.external_id)).collect();
    check_ownership(db, user.user_id, &account_ids).await?;

    let stored_accts = stored_accounts(db, &account_ids).await?;
    let tx_ids: Vec<Uuid> = rows.iter().map(|r| transaction_uuid(SOURCE_C24, &r.external_id)).collect();
    let stored_txs = stored_transaction_payloads(db, &tx_ids).await?;

    let imported_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let meta = |source_account_id| RecordMeta {
        source: SOURCE_C24,
        source_account_id,
        origin: ORIGIN_SOURCE,
        schema_version: CURRENT_SCHEMA_VERSION,
        imported_at: &imported_at,
        context: SourceContext::Unattributed,
    };

    let mut published: Vec<Published> = Vec::new();
    let mut failure: Option<async_graphql::Error> = None;

    // Accounts first, so a transaction lands on its real account rather than a stub.
    for (record, id) in accounts.iter().zip(&account_ids) {
        let new = serde_json::to_value(record).map_err(|_| serialize_failed())?;
        let change = match stored_accts.get(id) {
            // A stub (made by a transaction that arrived first) was never seen.
            Some((origin, _)) if origin == "stub" => Change::New,
            Some((_, payload)) => classify(payload.as_ref(), &new),
            None => Change::New,
        };
        if change == Change::Unchanged {
            continue;
        }
        let payload = serde_json::to_vec(record).map_err(|_| serialize_failed())?;
        if let Err(e) = publisher.publish(TOPIC_C24_ACCOUNT, record.topic_key(), &payload, &meta(None)).await {
            failure = Some(publish_failed(TOPIC_C24_ACCOUNT, e));
            break;
        }
        match change {
            Change::New => result.new_accounts += 1,
            _ => result.updated_accounts += 1,
        }
        published.push(Published {
            topic: TOPIC_C24_ACCOUNT,
            key: record.topic_key().to_string(),
            payload,
            source_account_id: None,
        });
    }

    if failure.is_none() {
        for (row, id) in rows.iter().zip(&tx_ids) {
            let record = C24TransactionRecord::from_row(row);
            let new = serde_json::to_value(&record).map_err(|_| serialize_failed())?;
            let change = classify(stored_txs.get(id), &new);
            if change == Change::Unchanged {
                result.unchanged_transactions += 1;
                continue;
            }
            let payload = serde_json::to_vec(&record).map_err(|_| serialize_failed())?;
            let rm = meta(Some(&row.account_number));
            if let Err(e) = publisher.publish(TOPIC_C24_TRANSACTION, record.topic_key(), &payload, &rm).await {
                failure = Some(publish_failed(TOPIC_C24_TRANSACTION, e));
                break;
            }
            match change {
                Change::New => result.new_transactions += 1,
                _ => result.updated_transactions += 1,
            }
            published.push(Published {
                topic: TOPIC_C24_TRANSACTION,
                key: record.topic_key().to_string(),
                payload,
                source_account_id: Some(row.account_number.clone()),
            });
        }
    }

    // Project whatever reached the broker, even when a later publish failed,
    // so the read model never lags our own acknowledged writes. Linking runs
    // on every upload, published or not, so a re-upload repairs a missing link.
    let imported_dt: DateTime<Utc> = DateTime::parse_from_rfc3339(&imported_at)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let txn = db.begin().await?;
    for p in &published {
        let input = MapperInput {
            event: SourceEvent {
                source: SOURCE_C24,
                source_account_id: p.source_account_id.as_deref(),
                key: &p.key,
                payload: &p.payload,
                imported_at: imported_dt,
            },
            comdirect_account_key: None,
            comdirect_account_name: None,
        };
        let outcome = if p.topic == TOPIC_C24_ACCOUNT {
            apply_account(&txn, &C24Mapper, &input, None).await
        } else {
            apply_transaction(&txn, &C24Mapper, &input, None).await
        };
        match outcome {
            Ok(()) => {}
            Err(ApplyError::Db(e)) => return Err(e.into()),
            // The bytes were just built by us, so this is a bug, not bad
            // input. The mapper's error text is static; still, say nothing more.
            Err(ApplyError::Map(_)) => {
                return Err(async_graphql::Error::new("a published C24 record could not be projected"))
            }
        }
    }
    let existing: HashSet<Uuid> = account::Entity::find()
        .filter(account::Column::Id.is_in(account_ids.clone()))
        .all(&txn)
        .await?
        .into_iter()
        .map(|a| a.id)
        .collect();
    for id in account_ids.iter().filter(|id| existing.contains(id)) {
        link_default_owner(&txn, user.user_id, *id, imported_dt).await?;
    }
    txn.commit().await?;

    match failure {
        Some(e) => Err(e),
        None => Ok(result),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kafka::c24::test_support::{card_row, transfer_row};
    use serde_json::json;

    const HEADER: &str = "Transaktionstyp,Buchungsdatum,Karteneinsatz,Betrag,Zahlungsempfänger,IBAN,BIC,Verwendungszweck,Beschreibung,Kontonummer,Kontoname,Kategorie,Unterkategorie,Bargeldabhebung";
    const CARD: &str = r#"Kartenzahlung,04.03.2026,03.03.2026 18:42,"-1,00 €",Beispiel Baeckerei,,,,,0000012345,Testkonto,Essen,Baeckerei"#;

    fn file(rows: &[&str]) -> String {
        let mut s = HEADER.to_string();
        for r in rows {
            s.push('\n');
            s.push_str(r);
        }
        s
    }

    #[test]
    fn classify_distinguishes_new_unchanged_and_updated() {
        let a = json!({"x": 1});
        assert_eq!(classify(None, &a), Change::New);
        assert_eq!(classify(Some(&a), &a), Change::Unchanged);
        assert_eq!(classify(Some(&json!({"x": 2})), &a), Change::Updated);
    }

    #[test]
    fn a_recategorised_row_keeps_its_id_but_classifies_as_updated() {
        let mut row = card_row();
        let before = serde_json::to_value(C24TransactionRecord::from_row(&row)).unwrap();
        row.category = None;
        let after = serde_json::to_value(C24TransactionRecord::from_row(&row)).unwrap();
        assert_eq!(classify(Some(&before), &after), Change::Updated);
    }

    #[test]
    fn one_account_record_per_account_number() {
        let rows = vec![card_row(), transfer_row()];
        assert_eq!(account_records(&rows).len(), 1);
    }

    #[test]
    fn a_bad_file_is_rejected_whole_and_named_by_position() {
        let files = vec![file(&[CARD]), "not,a,c24,export\n1,2,3,4".to_string(), file(&[CARD])];
        let (results, rows) = parse_files(&files).unwrap();
        assert_eq!(results.len(), 3);
        assert!(results[0].error.is_none() && results[2].error.is_none());
        assert_eq!(results[1].index, 1);
        assert_eq!(results[1].row_count, 0);
        assert!(results[1].error.is_some());
        // The good files still import, and the shared row is held once.
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn a_bad_cell_rejects_the_file_without_echoing_the_cell() {
        let secret = "SECRETPAYEE-NOT-A-NUMBER";
        let bad = format!(
            r#"Kartenzahlung,04.03.2026,,"{secret}",Beispiel Baeckerei,,,,,0000012345,Testkonto,,"#
        );
        let (results, rows) = parse_files(&[file(&[&bad])]).unwrap();
        assert!(rows.is_empty());
        let error = results[0].error.clone().expect("rejected");
        assert!(!error.contains(secret), "{error}");
        assert!(!error.contains("0000012345"), "{error}");
        assert!(error.contains("Betrag"), "{error}");
    }

    #[test]
    fn overlapping_files_merge_to_the_union() {
        let other = r#"Echtzeit-Überweisung,05.03.2026,,"1.500,00 €",Max Mustermann,DE00000000000000000000,TESTDE00XXX,Miete Maerz,,0000012345,Testkonto,,"#;
        let (_, rows) = parse_files(&[file(&[CARD]), file(&[CARD, other])]).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn limits_are_enforced() {
        assert!(parse_files(&[]).is_err());
        assert!(parse_files(&vec![file(&[CARD]); MAX_FILES + 1]).is_err());
        let (results, _) = parse_files(&["x".repeat(MAX_FILE_BYTES + 1)]).unwrap();
        assert!(results[0].error.is_some());
    }
}
