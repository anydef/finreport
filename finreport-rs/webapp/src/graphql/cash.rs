//! Cash accounts: the mutations behind `finreport.cash-account` and
//! `finreport.cash-transaction` (storage and projection: `kafka::cash`,
//! `projection::cash`).
//!
//! # Not admin-gated, on purpose
//!
//! A cash account is the caller's own data, like a goal or a display alias.
//! The `require_admin` resolvers in `current_user.rs` are for shared state
//! (rules, taxonomy, users). Do not "fix" this.
//!
//! # Scoping: owner AND link, and one refusal for everything else
//!
//! Every mutation that names an account (or a transaction, via its account)
//! resolves it through [`owned_cash_account`], which demands all of:
//!
//! * the row is `source = 'cash'` - so none of these mutations can reach an
//!   imported bank account, even one the caller owns (`deleteCashAccount`
//!   would otherwise delete bank data);
//! * the caller is linked to it in `user_account`;
//! * the caller is the owner **named in the account's own record**.
//!
//! The third check is not redundant. `user_account` is also written by
//! `user-admin link --all` and the admin bootstrap, which link *every*
//! account to the admin, so a link alone would let the admin rename or delete
//! another user's wallet - and a rename republishes the record, which would
//! hand the wallet to the admin on the next replay. The record's `user_id` is
//! what the projection itself treats as the owner.
//!
//! It reads the database rather than `AuthenticatedUser::account_ids`, which
//! is loaded once at the start of the request: a single operation that
//! creates an account and then adds a transaction to it must see the link
//! the first step just made.
//!
//! A missing id, a bank account, someone else's account and an unlinked one
//! all return the same `NOT_FOUND`, so the message never reveals whether an
//! account with that id exists.
//!
//! # Publish, then project
//!
//! As `uploadC24Transactions`: records go out through [`RecordMeta`] with the
//! ack-awaiting `EventPublisher::publish` (a lost cash record is data loss),
//! then the request applies them through `projection::cash::apply_record` -
//! the exact function the projector uses, on the exact bytes published - in
//! one DB transaction. The projector later re-applies them idempotently.
//!
//! # No revision
//!
//! The cash records carry no `revision`: a republish of the same key simply
//! replaces the previous one (compaction keeps the latest by log order, and
//! the projection's upsert guard lets `origin = 'user'` rows be overwritten).
//! There is one writer per record - its owner - so there is nothing to
//! order against.
//!
//! # No value in an error
//!
//! Amount, description and counterparty are the user's own financial data.
//! Validation errors name a field and a limit, never what was sent, and
//! storage failures are logged with their class and returned as a static
//! message.

use async_graphql::{ErrorExtensions, InputObject};
use chrono::{DateTime, SecondsFormat, Utc};
use entity::entities::{account, transaction, user_account};
use rust_decimal::Decimal as RustDecimal;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect, TransactionTrait,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::graphql::current_user::AuthenticatedUser;
use crate::graphql::events::{kafka_unavailable_error, publish_tombstone};
use crate::graphql::scalars::{Date, Decimal as GqlDecimal};
use crate::graphql::types::{Account, Transaction};
use crate::kafka::cash::{
    CashAccountRecord, CashTransactionRecord, CURRENT_SCHEMA_VERSION, SOURCE_CASH,
    TOPIC_CASH_ACCOUNT, TOPIC_CASH_TRANSACTION,
};
use crate::kafka::envelope::{account_uuid, transaction_uuid, Envelope, RecordMeta, SourceContext};
use crate::kafka::labeling::ORIGIN_USER;
use crate::kafka::producer::EventPublisher;
use crate::projection::mapper::MapperRegistry;
use crate::projection::{ApplyError, ConsumedRecord};

pub const MAX_LABEL_CHARS: usize = 100;
pub const MAX_DESCRIPTION_CHARS: usize = 500;
pub const MAX_COUNTERPARTY_CHARS: usize = 200;
/// `transaction.amount` is `numeric(20,4)`.
pub const MAX_AMOUNT_SCALE: u32 = 4;
/// Far below the column's range; a cash entry beyond this is a typo.
pub const MAX_AMOUNT_ABS: i64 = 1_000_000_000_000;

/// The fields of a cash transaction the user types in. `updateCashTransaction`
/// takes the same input and **replaces** all of them.
#[derive(InputObject, Clone, Debug)]
pub struct CashTransactionInput {
    /// The day the money moved.
    pub booking_date: Date,
    /// Signed: negative is money spent, positive is money received. Not zero.
    /// At most four decimal places.
    pub amount: GqlDecimal,
    /// ISO 4217 code, e.g. `EUR`. Must equal the account's currency: a cash
    /// account is single-currency, and its balance is the running sum of its
    /// transactions.
    pub currency: String,
    /// What it was for. `null` or blank stores none.
    pub description: Option<String>,
    /// Who was paid, or who paid. `null` or blank stores none.
    pub counterparty_name: Option<String>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn validation_error(message: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(message.into()).extend_with(|_, e| e.set("code", "VALIDATION"))
}

/// The single refusal for an account that does not exist, is not cash, or is
/// not the caller's. Deliberately says nothing that tells these apart.
fn account_not_found() -> async_graphql::Error {
    async_graphql::Error::new("cash account not found")
        .extend_with(|_, e| e.set("code", "NOT_FOUND"))
}

fn transaction_not_found() -> async_graphql::Error {
    async_graphql::Error::new("cash transaction not found")
        .extend_with(|_, e| e.set("code", "NOT_FOUND"))
}

fn publish_failed(topic: &str, e: rdkafka::error::KafkaError) -> async_graphql::Error {
    // `KafkaError`'s text names the broker condition, never the payload.
    async_graphql::Error::new(format!("failed to publish to {topic}: {e}"))
        .extend_with(|_, ext| ext.set("code", "KAFKA_PUBLISH_FAILED"))
}

fn storage_failed() -> async_graphql::Error {
    async_graphql::Error::new("could not store the change")
}

fn db_failed(e: sea_orm::DbErr) -> async_graphql::Error {
    // A `DbErr` can quote the offending row, so only its class is kept.
    tracing::error!(error_class = %db_err_class(&e), "cash: database error");
    storage_failed()
}

fn db_err_class(e: &sea_orm::DbErr) -> &'static str {
    match e {
        sea_orm::DbErr::ConnectionAcquire(_) | sea_orm::DbErr::Conn(_) => "connection",
        sea_orm::DbErr::Exec(_) => "exec",
        sea_orm::DbErr::Query(_) => "query",
        _ => "other",
    }
}

// ---------------------------------------------------------------------------
// Validation (pure)
// ---------------------------------------------------------------------------

/// Trims, and refuses blank or over-long text. `field` is the only thing the
/// error says.
pub fn required_text(field: &str, value: &str, max: usize) -> async_graphql::Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(validation_error(format!("{field} must not be blank")));
    }
    if trimmed.chars().count() > max {
        return Err(validation_error(format!("{field} must be at most {max} characters")));
    }
    Ok(trimmed.to_string())
}

/// Trims; blank becomes `None`; over-long is refused.
pub fn optional_text(
    field: &str,
    value: Option<&str>,
    max: usize,
) -> async_graphql::Result<Option<String>> {
    let Some(trimmed) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if trimmed.chars().count() > max {
        return Err(validation_error(format!("{field} must be at most {max} characters")));
    }
    Ok(Some(trimmed.to_string()))
}

/// Three ASCII letters, upper-cased.
pub fn currency_code(value: &str) -> async_graphql::Result<String> {
    let v = value.trim();
    if v.len() == 3 && v.chars().all(|c| c.is_ascii_alphabetic()) {
        Ok(v.to_ascii_uppercase())
    } else {
        Err(validation_error("currency must be a three-letter code"))
    }
}

/// Non-zero, at most [`MAX_AMOUNT_SCALE`] decimal places, below [`MAX_AMOUNT_ABS`].
pub fn amount(value: RustDecimal) -> async_graphql::Result<RustDecimal> {
    if value.is_zero() {
        return Err(validation_error("amount must not be zero"));
    }
    if value.normalize().scale() > MAX_AMOUNT_SCALE {
        return Err(validation_error(format!(
            "amount must have at most {MAX_AMOUNT_SCALE} decimal places"
        )));
    }
    if value.abs() >= RustDecimal::from(MAX_AMOUNT_ABS) {
        return Err(validation_error("amount is out of range"));
    }
    Ok(value)
}

/// Builds the record for a transaction on an account of `account_currency`.
pub fn build_transaction_record(
    external_id: String,
    account_external_id: &str,
    account_currency: &str,
    input: &CashTransactionInput,
) -> async_graphql::Result<CashTransactionRecord> {
    let currency = currency_code(&input.currency)?;
    if currency != account_currency {
        return Err(validation_error("currency must match the account's currency"));
    }
    Ok(CashTransactionRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        external_id,
        account_external_id: account_external_id.to_string(),
        booking_date: input.booking_date.0,
        amount: amount(input.amount.0)?,
        currency,
        counterparty_name: optional_text(
            "counterpartyName",
            input.counterparty_name.as_deref(),
            MAX_COUNTERPARTY_CHARS,
        )?,
        description: optional_text("description", input.description.as_deref(), MAX_DESCRIPTION_CHARS)?,
    })
}

// ---------------------------------------------------------------------------
// Scoping
// ---------------------------------------------------------------------------

/// The cash account `account_id`, if and only if it is a `cash` row linked to
/// the caller and owned by them per its own record (see the module docs).
async fn owned_cash_account(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    account_id: Uuid,
) -> async_graphql::Result<account::Model> {
    let row = account::Entity::find_by_id(account_id)
        .filter(account::Column::Source.eq(SOURCE_CASH))
        .one(db)
        .await
        .map_err(db_failed)?
        .ok_or_else(account_not_found)?;
    let linked = user_account::Entity::find()
        .filter(user_account::Column::UserId.eq(user.user_id))
        .filter(user_account::Column::AccountId.eq(account_id))
        .one(db)
        .await
        .map_err(db_failed)?
        .is_some();
    // A stub (no record yet) has no owner, so it is nobody's.
    let owner = row
        .raw_payload
        .clone()
        .and_then(|p| serde_json::from_value::<CashAccountRecord>(p).ok())
        .map(|r| r.user_id);
    if linked && owner == Some(user.user_id) {
        Ok(row)
    } else {
        Err(account_not_found())
    }
}

/// The cash transaction `transaction_id` and its account, scoped as above.
async fn owned_cash_transaction(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    transaction_id: Uuid,
) -> async_graphql::Result<(transaction::Model, account::Model)> {
    let tx = transaction::Entity::find_by_id(transaction_id)
        .filter(transaction::Column::Source.eq(SOURCE_CASH))
        .one(db)
        .await
        .map_err(db_failed)?
        .ok_or_else(transaction_not_found)?;
    let acct = owned_cash_account(db, user, tx.account_id)
        .await
        .map_err(|_| transaction_not_found())?;
    Ok((tx, acct))
}

// ---------------------------------------------------------------------------
// Publish and project
// ---------------------------------------------------------------------------

/// Publishes one cash record, then projects it.
async fn publish_and_project(
    db: &DatabaseConnection,
    publisher: &Arc<EventPublisher>,
    topic: &'static str,
    key: &str,
    payload: Vec<u8>,
    source_account_id: Option<&str>,
) -> async_graphql::Result<()> {
    let imported_at: DateTime<Utc> = Utc::now();
    let stamp = imported_at.to_rfc3339_opts(SecondsFormat::Secs, true);
    let meta = RecordMeta {
        source: SOURCE_CASH,
        source_account_id,
        origin: ORIGIN_USER,
        schema_version: CURRENT_SCHEMA_VERSION,
        imported_at: &stamp,
        context: SourceContext::Unattributed,
    };
    publisher
        .publish(topic, key, &payload, &meta)
        .await
        .map_err(|e| publish_failed(topic, e))?;
    let envelope = Envelope {
        source: SOURCE_CASH.to_string(),
        source_account_id: source_account_id.map(str::to_string),
        origin: ORIGIN_USER.to_string(),
        schema_version: CURRENT_SCHEMA_VERSION,
        imported_at,
        comdirect_account_key: None,
        comdirect_account_name: None,
    };
    project(db, topic, key, payload, envelope).await
}

/// Applies a record (an empty payload is a tombstone) the way the projector
/// does, in one DB transaction.
async fn project(
    db: &DatabaseConnection,
    topic: &str,
    key: &str,
    payload: Vec<u8>,
    envelope: Envelope,
) -> async_graphql::Result<()> {
    let record = ConsumedRecord {
        topic: topic.to_string(),
        partition: 0,
        // Not a consumed offset; `apply_record` reads it only to log.
        offset: -1,
        key: Some(key.to_string()),
        payload,
        envelope,
    };
    let registry = MapperRegistry::with_default_mappers();
    let txn = db.begin().await.map_err(db_failed)?;
    match crate::projection::cash::apply_record(&txn, &registry, &record).await {
        Ok(()) => {}
        Err(ApplyError::Db(e)) => return Err(db_failed(e)),
        // The bytes were just built by us, so this is a bug, not bad input.
        Err(ApplyError::Map(_)) => {
            tracing::error!("cash: a record we published could not be projected");
            return Err(storage_failed());
        }
    }
    txn.commit().await.map_err(db_failed)?;
    Ok(())
}

/// What a tombstone looks like to the projector: no headers, so the parser's
/// defaults.
fn tombstone_envelope() -> Envelope {
    Envelope {
        source: "comdirect".to_string(),
        source_account_id: None,
        origin: "source".to_string(),
        schema_version: 1,
        imported_at: Utc::now(),
        comdirect_account_key: None,
        comdirect_account_name: None,
    }
}

fn serialize<T: serde::Serialize>(record: &T) -> async_graphql::Result<Vec<u8>> {
    serde_json::to_vec(record)
        .map_err(|_| async_graphql::Error::new("failed to serialize a cash record"))
}

async fn reload_account(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    id: Uuid,
) -> async_graphql::Result<Account> {
    crate::graphql::accounts::fetch_accounts(db, user.user_id, &[id])
        .await?
        .into_iter()
        .next()
        .ok_or_else(storage_failed)
}

async fn reload_transaction(db: &DatabaseConnection, id: Uuid) -> async_graphql::Result<Transaction> {
    let row = transaction::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(db_failed)?
        .ok_or_else(storage_failed)?;
    Ok(crate::graphql::transactions::to_graphql_transaction(row))
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

pub async fn create_cash_account(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    label: String,
    currency: String,
) -> async_graphql::Result<Account> {
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let record = CashAccountRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        // Generated, never derived from the label: see `kafka::cash`.
        external_id: Uuid::new_v4().to_string(),
        user_id: user.user_id,
        label: required_text("label", &label, MAX_LABEL_CHARS)?,
        currency: currency_code(&currency)?,
    };
    let payload = serialize(&record)?;
    publish_and_project(db, publisher, TOPIC_CASH_ACCOUNT, record.topic_key(), payload, None)
        .await?;
    reload_account(db, user, account_uuid(SOURCE_CASH, &record.external_id)).await
}

pub async fn rename_cash_account(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    account_id: Uuid,
    label: String,
) -> async_graphql::Result<Account> {
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let row = owned_cash_account(db, user, account_id).await?;
    let record = CashAccountRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        external_id: row.external_id,
        user_id: user.user_id,
        label: required_text("label", &label, MAX_LABEL_CHARS)?,
        currency: row.currency,
    };
    let payload = serialize(&record)?;
    publish_and_project(db, publisher, TOPIC_CASH_ACCOUNT, record.topic_key(), payload, None)
        .await?;
    reload_account(db, user, account_id).await
}

/// Tombstones every transaction the account owns, then the account (last, so
/// a failure part-way leaves the account in place and the call retryable),
/// then removes them from the read model.
pub async fn delete_cash_account(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    account_id: Uuid,
) -> async_graphql::Result<bool> {
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let row = owned_cash_account(db, user, account_id).await?;
    let tx_keys: Vec<String> = transaction::Entity::find()
        .select_only()
        .column(transaction::Column::ExternalId)
        .filter(transaction::Column::AccountId.eq(account_id))
        .into_tuple()
        .all(db)
        .await
        .map_err(db_failed)?;
    // The compacted transaction topic would otherwise keep every record, and a
    // replay would project them under a stub account.
    for key in &tx_keys {
        publish_tombstone(publisher, TOPIC_CASH_TRANSACTION, key).await?;
    }
    publish_tombstone(publisher, TOPIC_CASH_ACCOUNT, &row.external_id).await?;
    // The account tombstone cascades to its transactions in the read model.
    project(db, TOPIC_CASH_ACCOUNT, &row.external_id, Vec::new(), tombstone_envelope()).await?;
    Ok(true)
}

pub async fn create_cash_transaction(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    account_id: Uuid,
    input: CashTransactionInput,
) -> async_graphql::Result<Transaction> {
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let acct = owned_cash_account(db, user, account_id).await?;
    let record = build_transaction_record(
        Uuid::new_v4().to_string(),
        &acct.external_id,
        &acct.currency,
        &input,
    )?;
    let payload = serialize(&record)?;
    publish_and_project(
        db,
        publisher,
        TOPIC_CASH_TRANSACTION,
        record.topic_key(),
        payload,
        Some(&acct.external_id),
    )
    .await?;
    reload_transaction(db, transaction_uuid(SOURCE_CASH, &record.external_id)).await
}

pub async fn update_cash_transaction(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    transaction_id: Uuid,
    input: CashTransactionInput,
) -> async_graphql::Result<Transaction> {
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let (tx, acct) = owned_cash_transaction(db, user, transaction_id).await?;
    let record =
        build_transaction_record(tx.external_id, &acct.external_id, &acct.currency, &input)?;
    let payload = serialize(&record)?;
    publish_and_project(
        db,
        publisher,
        TOPIC_CASH_TRANSACTION,
        record.topic_key(),
        payload,
        Some(&acct.external_id),
    )
    .await?;
    reload_transaction(db, transaction_id).await
}

pub async fn delete_cash_transaction(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    transaction_id: Uuid,
) -> async_graphql::Result<bool> {
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let (tx, _) = owned_cash_transaction(db, user, transaction_id).await?;
    publish_tombstone(publisher, TOPIC_CASH_TRANSACTION, &tx.external_id).await?;
    // Also un-pairs a bank-side transfer leg that pointed at this row.
    project(db, TOPIC_CASH_TRANSACTION, &tx.external_id, Vec::new(), tombstone_envelope()).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dec(s: &str) -> RustDecimal {
        s.parse().unwrap()
    }

    fn input() -> CashTransactionInput {
        CashTransactionInput {
            booking_date: Date(NaiveDate::from_ymd_opt(2026, 3, 4).unwrap()),
            amount: GqlDecimal(dec("-4.50")),
            currency: "eur".into(),
            description: Some("  Broetchen ".into()),
            counterparty_name: Some("   ".into()),
        }
    }

    #[test]
    fn a_valid_input_is_normalised() {
        let r = build_transaction_record("id".into(), "acct", "EUR", &input()).unwrap();
        assert_eq!(r.currency, "EUR");
        assert_eq!(r.description.as_deref(), Some("Broetchen"));
        assert_eq!(r.counterparty_name, None, "blank is no counterparty");
        assert_eq!(r.account_external_id, "acct");
    }

    #[test]
    fn a_currency_other_than_the_accounts_is_refused() {
        let err = build_transaction_record("id".into(), "acct", "USD", &input()).unwrap_err();
        assert!(err.message.contains("currency"));
    }

    #[test]
    fn labels_and_currencies_are_validated() {
        assert_eq!(required_text("label", "  Wallet ", 100).unwrap(), "Wallet");
        assert!(required_text("label", "   ", 100).is_err());
        assert!(required_text("label", &"x".repeat(101), 100).is_err());
        assert_eq!(currency_code(" eur ").unwrap(), "EUR");
        for bad in ["", "EU", "EURO", "E1R", "€€€"] {
            assert!(currency_code(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn amounts_are_bounded() {
        assert!(amount(dec("0")).is_err());
        assert!(amount(dec("0.00")).is_err());
        assert!(amount(dec("1.23456")).is_err());
        assert!(amount(dec("1.2300")).is_ok(), "trailing zeros are not precision");
        assert!(amount(dec("1000000000000")).is_err());
        assert!(amount(dec("-999999999999.99")).is_ok());
    }

    #[test]
    fn errors_name_a_field_never_a_value() {
        let secret = "SECRET-PURPOSE-9876";
        let mut i = input();
        i.description = Some(secret.repeat(100));
        let mut errors = vec![build_transaction_record("id".into(), "a", "EUR", &i).unwrap_err()];
        let mut i = input();
        i.currency = secret.into();
        errors.push(build_transaction_record("id".into(), "a", "EUR", &i).unwrap_err());
        let mut i = input();
        i.counterparty_name = Some(secret.repeat(100));
        errors.push(build_transaction_record("id".into(), "a", "EUR", &i).unwrap_err());
        let mut i = input();
        i.amount = GqlDecimal(dec("7777777.123456"));
        errors.push(build_transaction_record("id".into(), "a", "EUR", &i).unwrap_err());
        errors.push(required_text("label", &secret.repeat(100), 100).unwrap_err());
        for e in errors {
            assert!(!e.message.contains("SECRET") && !e.message.contains("7777777"), "{}", e.message);
        }
    }

    #[test]
    fn tombstones_use_the_headerless_defaults() {
        let e = tombstone_envelope();
        assert_eq!((e.source.as_str(), e.origin.as_str()), ("comdirect", "source"));
    }
}
