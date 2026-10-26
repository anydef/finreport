//! Cash accounts: `CashMapper` for `(cash, user)` plus the projection of the
//! two cash topics ([`crate::kafka::cash`]), including **tombstones**.
//!
//! # Not the generic ingest path
//!
//! Cash records are routed here by `process_batch` instead of through
//! `apply_account` / `apply_transaction`, for three reasons that all follow
//! from "this is user-authored data, not a bank's":
//!
//! * a cash account is linked to the user **named in its own record**, never
//!   to `APP_projector_default_owner` - handing every user's wallet to the
//!   deployment's default owner would leak it;
//! * a tombstone carries no headers (so no `(source, origin)` to resolve a
//!   mapper from) and no payload; the id comes from the topic and the key;
//! * deleting has consequences beyond the row itself (below).
//!
//! # What a tombstone removes
//!
//! Deleting a cash **transaction** removes the row and everything projected
//! *about* it: its `transaction_insight`, label, user label, tags, splits and
//! link memberships (a link left with no members goes too). Deleting a cash
//! **account** does that for every transaction it owns, then removes the
//! account's savings flag, its `user_account` links and the account row.
//!
//! # The dangling-transfer trap
//!
//! `transaction_insight.transfer_counterpart_id` holds the other side of a
//! detected transfer, and the detector flags **both** rows. If a cash
//! transaction paired with a bank transaction is deleted, the bank leg would
//! stay `is_transfer = true` pointing at a row that no longer exists - and
//! the cashflow aggregations read that flag, so the bank leg would silently
//! vanish from the totals. [`clear_transfer_pairings`] therefore un-pairs
//! every surviving row that pointed at a deleted one.
//!
//! **Only the transfer columns are cleared, not the whole insight row.** The
//! recurring-series columns describe the bank leg on its own history (the
//! cash row never belonged to that series, being on another account), so
//! wiping them would throw away a still-true fact for no reason. The row's
//! `revision` is left alone: it is the detector's guard against replaying a
//! stale record, and this is a consequence of a deletion, not a new detector
//! verdict.
//!
//! Known limit: this fixes the *projection*. The old `transaction-insight`
//! record for the bank leg still sits on its compacted topic, so a full
//! replay may project it before the tombstone is reached; the detector's
//! next run, which compares against what it sees, re-derives the unpaired
//! state. The delete mutation (a later slice) should tombstone the account's
//! transactions as well as the account for the same reason: a compacted topic
//! keeps a deleted account's transaction records, and a replay would
//! otherwise project them under a stub account.
//!
//! # No value in an error
//!
//! Amount and description are the user's own financial data. Like the C24
//! mapper, this one never constructs `UnparseableDate` / `UnparseableAmount`
//! and never forwards a serde message: every failure is `InvalidPayload` with
//! a static string.

use entity::entities::{
    account, account_savings, app_user, transaction, transaction_insight, transaction_label,
    transaction_link, transaction_link_member, transaction_split, transaction_tag,
    transaction_user_label, user_account,
};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use serde_json::Value;
use tracing::warn;
use uuid::Uuid;

use crate::kafka::cash::{
    CashAccountRecord, CashTransactionRecord, SOURCE_CASH, TOPIC_CASH_ACCOUNT,
    TOPIC_CASH_TRANSACTION,
};
use crate::kafka::envelope::{account_uuid, transaction_uuid};
use crate::kafka::labeling::ORIGIN_USER;
use crate::projection::mapper::{MapperInput, MapperRegistry, SourceMapper};
use crate::projection::records::{AccountRecord, BalanceRecord, MapError, TransactionRecord};
use crate::projection::{apply_transaction, ApplyError, ConsumedRecord};

pub struct CashMapper;

const BAD_ACCOUNT: &str = "malformed cash account record";
const BAD_TRANSACTION: &str = "malformed cash transaction record";
const ACCOUNT_MISMATCH: &str = "cash transaction names a different account than its header";

fn parse<T: serde::de::DeserializeOwned>(
    payload: &[u8],
    reason: &'static str,
) -> Result<(T, Value), MapError> {
    let record =
        serde_json::from_slice(payload).map_err(|_| MapError::InvalidPayload(reason.to_string()))?;
    let raw =
        serde_json::from_slice(payload).map_err(|_| MapError::InvalidPayload(reason.to_string()))?;
    Ok((record, raw))
}

/// The owner named by a cash account record - what links `user_account`.
pub fn account_owner(payload: &[u8]) -> Result<Uuid, MapError> {
    let (rec, _): (CashAccountRecord, Value) = parse(payload, BAD_ACCOUNT)?;
    Ok(rec.user_id)
}

impl SourceMapper for CashMapper {
    fn source(&self) -> &'static str {
        SOURCE_CASH
    }

    fn map_account(&self, input: &MapperInput<'_>) -> Result<AccountRecord, MapError> {
        let (rec, raw): (CashAccountRecord, Value) = parse(input.event.payload, BAD_ACCOUNT)?;
        Ok(AccountRecord {
            id: account_uuid(input.event.source, &rec.external_id),
            source: input.event.source.to_string(),
            display_id: None,
            account_type: None,
            // A wallet has no IBAN; the column is nullable.
            iban: None,
            bic: None,
            institute: None,
            label: Some(rec.label),
            currency: rec.currency,
            external_id: rec.external_id,
            raw_payload: Some(raw),
            origin: ORIGIN_USER.to_string(),
            first_seen_at: input.event.imported_at,
            updated_at: input.event.imported_at,
        })
    }

    /// No bank reports a balance for a cash account; it is derived from the
    /// account's own transactions on read. Inventing a row would put a
    /// made-up number in the read model.
    fn map_balance(&self, _input: &MapperInput<'_>) -> Result<BalanceRecord, MapError> {
        Err(MapError::InvalidPayload(
            "cash accounts have no balance records".to_string(),
        ))
    }

    fn map_transaction(&self, input: &MapperInput<'_>) -> Result<TransactionRecord, MapError> {
        let source_account_id = input
            .event
            .source_account_id
            .ok_or(MapError::MissingSourceAccountId)?;
        let (rec, raw): (CashTransactionRecord, Value) =
            parse(input.event.payload, BAD_TRANSACTION)?;
        if rec.account_external_id != source_account_id {
            return Err(MapError::InvalidPayload(ACCOUNT_MISMATCH.to_string()));
        }
        Ok(TransactionRecord {
            id: transaction_uuid(input.event.source, &rec.external_id),
            account_id: account_uuid(input.event.source, source_account_id),
            source: input.event.source.to_string(),
            external_id: rec.external_id,
            booking_date: rec.booking_date,
            valuta_date: None,
            booking_status: crate::kafka::cash::BOOKING_STATUS_BOOKED.to_string(),
            amount: rec.amount,
            currency: rec.currency,
            counterparty_name: rec.counterparty_name,
            counterparty_iban: None,
            description: rec.description,
            transaction_type: None,
            raw_payload: raw,
            origin: ORIGIN_USER.to_string(),
            imported_at: input.event.imported_at,
            updated_at: input.event.imported_at,
            // Filled by the labeler's processor, as for every source.
            counterparty_key: None,
        })
    }
}

/// Whether `topic` is one of the two cash topics `process_batch` routes here.
pub(crate) fn is_cash_topic(topic: &str) -> bool {
    topic == TOPIC_CASH_ACCOUNT || topic == TOPIC_CASH_TRANSACTION
}

/// Applies one record from a cash topic: an upsert, or - for a null payload -
/// a delete. Runs inside the batch transaction.
pub(crate) async fn apply_record(
    txn: &sea_orm::DatabaseTransaction,
    registry: &MapperRegistry,
    record: &ConsumedRecord,
) -> Result<(), ApplyError> {
    let is_account = record.topic == TOPIC_CASH_ACCOUNT;

    if record.payload.is_empty() {
        let Some(key) = record.key.as_deref() else {
            warn!(topic = %record.topic, offset = record.offset, "projector: cash tombstone with no key, skipped");
            return Ok(());
        };
        if is_account {
            delete_account(txn, account_uuid(SOURCE_CASH, key)).await?;
        } else {
            delete_transactions(txn, &[transaction_uuid(SOURCE_CASH, key)]).await?;
        }
        return Ok(());
    }

    let mapper = registry
        .resolve(&record.envelope.source, &record.envelope.origin)
        .filter(|m| m.source() == SOURCE_CASH)
        .ok_or_else(|| MapError::InvalidPayload("no cash mapper for this (source, origin)".to_string()))?;
    let input = MapperInput {
        event: crate::kafka::envelope::SourceEvent {
            source: &record.envelope.source,
            source_account_id: record.envelope.source_account_id.as_deref(),
            key: record.key.as_deref().unwrap_or(""),
            payload: &record.payload,
            imported_at: record.envelope.imported_at,
        },
        comdirect_account_key: None,
        comdirect_account_name: None,
    };

    if is_account {
        let mapped = mapper.map_account(&input)?;
        let owner = account_owner(&record.payload)?;
        crate::projection::upsert::upsert_account(txn, &mapped).await?;
        // The record names its owner, so the log alone rebuilds the link.
        // Idempotent (`do nothing` on conflict), so a replay is a no-op.
        //
        // `user_account.user_id` is a foreign key. A replay onto a database
        // that no longer has this user (deleted, or not yet created) must not
        // wedge the projector on a constraint error forever, so a missing
        // owner is logged and the account is projected unlinked.
        if app_user::Entity::find_by_id(owner).one(txn).await?.is_some() {
            crate::projection::upsert::link_default_owner(txn, owner, mapped.id, mapped.updated_at)
                .await?;
        } else {
            warn!(topic = %record.topic, offset = record.offset, "projector: cash account names an unknown user, projected without an owner link");
        }
        Ok(())
    } else {
        // `None` default owner on purpose: see the module docs.
        apply_transaction(txn, mapper, &input, None).await
    }
}

/// Deletes the given transactions and everything projected about them,
/// un-pairing any transfer they were part of first (see the module docs).
pub async fn delete_transactions(txn: &impl ConnectionTrait, ids: &[Uuid]) -> Result<(), DbErr> {
    if ids.is_empty() {
        return Ok(());
    }
    clear_transfer_pairings(txn, ids).await?;

    transaction_insight::Entity::delete_many()
        .filter(transaction_insight::Column::TransactionId.is_in(ids.to_vec()))
        .exec(txn)
        .await?;
    transaction_label::Entity::delete_many()
        .filter(transaction_label::Column::TransactionId.is_in(ids.to_vec()))
        .exec(txn)
        .await?;
    transaction_user_label::Entity::delete_many()
        .filter(transaction_user_label::Column::TransactionId.is_in(ids.to_vec()))
        .exec(txn)
        .await?;
    transaction_tag::Entity::delete_many()
        .filter(transaction_tag::Column::TransactionId.is_in(ids.to_vec()))
        .exec(txn)
        .await?;
    transaction_split::Entity::delete_many()
        .filter(transaction_split::Column::TransactionId.is_in(ids.to_vec()))
        .exec(txn)
        .await?;
    remove_from_links(txn, ids).await?;

    transaction::Entity::delete_many()
        .filter(transaction::Column::Id.is_in(ids.to_vec()))
        .exec(txn)
        .await?;
    Ok(())
}

/// Un-pairs every row whose `transfer_counterpart_id` is one of `deleted`:
/// clears the three transfer columns and nothing else (see the module docs
/// for why not the whole row).
async fn clear_transfer_pairings(txn: &impl ConnectionTrait, deleted: &[Uuid]) -> Result<(), DbErr> {
    transaction_insight::Entity::update_many()
        .col_expr(transaction_insight::Column::IsTransfer, Expr::value(false))
        .col_expr(
            transaction_insight::Column::TransferCounterpartId,
            Expr::value(Option::<Uuid>::None),
        )
        .col_expr(
            transaction_insight::Column::TransferMatch,
            Expr::value(Option::<String>::None),
        )
        .filter(transaction_insight::Column::TransferCounterpartId.is_in(deleted.to_vec()))
        .exec(txn)
        .await?;
    Ok(())
}

/// Removes the transactions from any reimbursement link; a link left with no
/// members at all is deleted, since a memberless link means nothing.
async fn remove_from_links(txn: &impl ConnectionTrait, ids: &[Uuid]) -> Result<(), DbErr> {
    let touched: Vec<Uuid> = transaction_link_member::Entity::find()
        .filter(transaction_link_member::Column::TransactionId.is_in(ids.to_vec()))
        .all(txn)
        .await?
        .into_iter()
        .map(|m| m.link_id)
        .collect();
    if touched.is_empty() {
        return Ok(());
    }
    transaction_link_member::Entity::delete_many()
        .filter(transaction_link_member::Column::TransactionId.is_in(ids.to_vec()))
        .exec(txn)
        .await?;
    for link_id in touched {
        let remaining = transaction_link_member::Entity::find()
            .filter(transaction_link_member::Column::LinkId.eq(link_id))
            .one(txn)
            .await?;
        if remaining.is_none() {
            transaction_link::Entity::delete_by_id(link_id).exec(txn).await?;
        }
    }
    Ok(())
}

/// Deletes a cash account: every transaction it owns (with the full
/// [`delete_transactions`] treatment), then its savings flags, its
/// `user_account` links and the row itself.
pub async fn delete_account(txn: &impl ConnectionTrait, account_id: Uuid) -> Result<(), DbErr> {
    let ids: Vec<Uuid> = transaction::Entity::find()
        .filter(transaction::Column::AccountId.eq(account_id))
        .all(txn)
        .await?
        .into_iter()
        .map(|t| t.id)
        .collect();
    delete_transactions(txn, &ids).await?;

    account_savings::Entity::delete_many()
        .filter(account_savings::Column::AccountId.eq(account_id))
        .exec(txn)
        .await?;
    user_account::Entity::delete_many()
        .filter(user_account::Column::AccountId.eq(account_id))
        .exec(txn)
        .await?;
    account::Entity::delete_by_id(account_id).exec(txn).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kafka::cash::test_support::{account, transaction, ACCOUNT_ID, USER_ID};
    use crate::kafka::envelope::SourceEvent;
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;

    fn input<'a>(
        source_account_id: Option<&'a str>,
        key: &'a str,
        payload: &'a [u8],
    ) -> MapperInput<'a> {
        MapperInput {
            event: SourceEvent {
                source: "cash",
                source_account_id,
                key,
                payload,
                imported_at: Utc.with_ymd_and_hms(2026, 3, 6, 9, 0, 0).unwrap(),
            },
            comdirect_account_key: None,
            comdirect_account_name: None,
        }
    }

    #[test]
    fn maps_a_transaction_record() {
        let rec = transaction();
        let bytes = serde_json::to_vec(&rec).unwrap();
        let out = CashMapper
            .map_transaction(&input(Some(ACCOUNT_ID), &rec.external_id, &bytes))
            .unwrap();
        assert_eq!(out.source, "cash");
        assert_eq!(out.id, transaction_uuid("cash", &rec.external_id));
        assert_eq!(out.account_id, account_uuid("cash", ACCOUNT_ID));
        assert_eq!(out.amount, Decimal::new(-450, 2));
        assert_eq!(out.currency, "EUR");
        assert_eq!(out.booking_status, "BOOKED");
        assert_eq!(out.counterparty_name.as_deref(), Some("Beispiel Baeckerei"));
        assert_eq!(out.counterparty_iban, None);
        assert_eq!(out.description.as_deref(), Some("Broetchen"));
        assert_eq!(out.origin, "user");
        assert_eq!(out.counterparty_key, None);
        assert_eq!(out.raw_payload["external_id"], rec.external_id);
    }

    #[test]
    fn maps_an_account_record_and_names_its_owner() {
        let rec = account();
        let bytes = serde_json::to_vec(&rec).unwrap();
        let out = CashMapper
            .map_account(&input(None, &rec.external_id, &bytes))
            .unwrap();
        assert_eq!(out.id, account_uuid("cash", ACCOUNT_ID));
        assert_eq!(out.label.as_deref(), Some("Wallet"));
        assert_eq!(out.iban, None);
        assert_eq!(out.origin, "user");
        assert_eq!(account_owner(&bytes).unwrap(), USER_ID);
    }

    #[test]
    fn transaction_without_source_account_id_is_rejected() {
        let rec = transaction();
        let bytes = serde_json::to_vec(&rec).unwrap();
        assert_eq!(
            CashMapper
                .map_transaction(&input(None, &rec.external_id, &bytes))
                .unwrap_err(),
            MapError::MissingSourceAccountId
        );
    }

    #[test]
    fn transaction_whose_header_disagrees_with_its_payload_is_rejected() {
        let rec = transaction();
        let bytes = serde_json::to_vec(&rec).unwrap();
        let err = CashMapper
            .map_transaction(&input(Some("some-other-account"), &rec.external_id, &bytes))
            .unwrap_err();
        assert!(matches!(err, MapError::InvalidPayload(_)));
        assert!(!err.to_string().contains("some-other-account"));
    }

    #[test]
    fn balance_is_refused_not_fabricated() {
        let err = CashMapper
            .map_balance(&input(Some(ACCOUNT_ID), "k", b"{}"))
            .unwrap_err();
        assert!(matches!(err, MapError::InvalidPayload(_)));
    }

    #[test]
    fn errors_never_carry_a_payload_value() {
        // Amount and description are the user's own financial data. Each
        // value is malformed or sits among malformed fields, so serde has
        // plenty of chances to quote one.
        let bad = br#"{"schema_version":1,"external_id":"SECRET-EXT-ID",
            "account_external_id":"SECRET-ACCOUNT","booking_date":"SECRET-DATE",
            "amount":"SECRET-AMOUNT","currency":"EUR",
            "counterparty_name":"SECRET-PAYEE","description":"SECRET-PURPOSE"}"#;
        let secrets = [
            "SECRET-EXT-ID",
            "SECRET-ACCOUNT",
            "SECRET-DATE",
            "SECRET-AMOUNT",
            "SECRET-PAYEE",
            "SECRET-PURPOSE",
        ];

        let tx = CashMapper
            .map_transaction(&input(Some(ACCOUNT_ID), "k", bad))
            .unwrap_err();
        let acct = CashMapper
            .map_account(&input(None, "k", br#"{"schema_version":"SECRET-AMOUNT"}"#))
            .unwrap_err();
        let owner = account_owner(br#"{"user_id":"SECRET-PAYEE"}"#).unwrap_err();
        let truncated = CashMapper
            .map_transaction(&input(Some(ACCOUNT_ID), "k", &bad[..60]))
            .unwrap_err();
        // A well-formed record whose account disagrees with the header.
        let good = serde_json::to_vec(&transaction()).unwrap();
        let mismatch = CashMapper
            .map_transaction(&input(Some("SECRET-ACCOUNT"), "k", &good))
            .unwrap_err();

        for err in [tx, acct, owner, truncated, mismatch] {
            assert!(matches!(err, MapError::InvalidPayload(_)));
            let text = err.to_string();
            for secret in secrets {
                assert!(!text.contains(secret), "error leaked {secret}: {text}");
            }
            assert!(!text.contains("Broetchen") && !text.contains("4.50"), "{text}");
        }
    }

    #[test]
    fn registry_resolves_cash_user_only() {
        let registry = MapperRegistry::with_default_mappers();
        assert!(registry.resolve("cash", "user").is_some());
        assert!(registry.resolve("cash", "source").is_none());
    }

    #[test]
    fn only_the_two_cash_topics_are_routed_here() {
        assert!(is_cash_topic(TOPIC_CASH_ACCOUNT));
        assert!(is_cash_topic(TOPIC_CASH_TRANSACTION));
        assert!(!is_cash_topic("finreport.c24-transaction"));
    }
}
