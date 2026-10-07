//! Postgres-only projector integration tests (§9 "done when"): replay
//! idempotency, reset+replay, precedence in both arrival orders, stub
//! accounts getting filled in, first-writer-owns-account-link, and a
//! mid-batch-crash simulation — all driven straight through
//! `projection::process_batch`, no Kafka involved.
//!
//! Gated behind the `integration` feature (`just test` never enables it) and
//! a real Docker daemon: starts a throwaway Postgres container (WP6's shared
//! `support::TestPostgres` harness — auto-named/auto-ported, so starting a
//! second one mid-test never races the first's teardown), removed again at
//! the end of each test run.
#![cfg(feature = "integration")]

#[path = "support/mod.rs"]
mod support;

use chrono::{TimeZone, Utc};
use entity::entities::{account, account_balance, transaction};
use rust_decimal::Decimal;
use sea_orm::EntityTrait;
use serde_json::json;
use std::str::FromStr;
use uuid::Uuid;
use webapp::kafka::envelope::Envelope;
use webapp::projection::mapper::MapperRegistry;
use webapp::projection::offsets::load_offsets;
use webapp::projection::upsert::offset_for;
use webapp::projection::{process_batch, ConsumedRecord};

const GROUP: &str = "default";
const ACCOUNT_TOPIC: &str = "finreport.account";
const BALANCE_TOPIC: &str = "finreport.account-balance";
const TRANSACTION_TOPIC: &str = "finreport.transaction";

fn envelope(source_account_id: Option<&str>, origin: &str) -> Envelope {
    Envelope {
        source: "comdirect".to_string(),
        source_account_id: source_account_id.map(str::to_string),
        origin: origin.to_string(),
        schema_version: 1,
        imported_at: Utc.with_ymd_and_hms(2024, 6, 15, 8, 30, 0).unwrap(),
        comdirect_account_key: Some("0".to_string()),
        comdirect_account_name: Some("Main".to_string()),
    }
}

fn record(
    topic: &str,
    offset: i64,
    key: &str,
    payload: serde_json::Value,
    source_account_id: Option<&str>,
    origin: &str,
) -> ConsumedRecord {
    ConsumedRecord {
        topic: topic.to_string(),
        partition: 0,
        offset,
        key: Some(key.to_string()),
        payload: serde_json::to_vec(&payload).unwrap(),
        envelope: envelope(source_account_id, origin),
    }
}

fn account_payload(account_id: &str) -> serde_json::Value {
    json!({
        "iban": "DE11500105171234567890",
        "bic": "COBADEFFXXX",
        "accountId": account_id,
        "accountDisplayId": "1053820100",
        "accountType": {"text": "Girokonto"}
    })
}

fn balance_payload(value: &str) -> serde_json::Value {
    json!({"value": value, "unit": "EUR"})
}

fn transaction_payload(reference: &str, amount: &str, holder_name: &str) -> serde_json::Value {
    json!({
        "reference": reference,
        "bookingStatus": "BOOKED",
        "bookingDate": "2024-01-02",
        "amount": {"value": amount, "unit": "EUR"},
        "remitter": {"holderName": holder_name},
        "deptor": null,
        "creditor": null,
        "valutaDate": "2024-01-02",
        "directDebitCreditorId": null,
        "directDebitMandateId": null,
        "endToEndReference": null,
        "newTransaction": false,
        "remittanceInfo": "test",
        "transactionType": {"key": "TRANSFER", "text": "Transfer"}
    })
}

fn legacy_transaction_payload(reference: &str, account_id: &str, amount: &str) -> serde_json::Value {
    json!({
        "account_id": account_id,
        "reference": reference,
        "booking_date": "2023-11-15",
        "valuta_date": "2023-11-15",
        "booking_status": "BOOKED",
        "amount": amount,
        "currency": "EUR",
        "remitter": "Old Employer",
        "remittance_info": "Legacy salary"
    })
}

#[tokio::test]
async fn replay_is_idempotent_and_reset_replay_reproduces_identical_state() {
    let db = support::TestPostgres::start().await;
    let registry = MapperRegistry::with_default_mappers();
    let fixtures = support::load_fixture_records();

    let first = process_batch(db.connection(), GROUP, &registry, None, &fixtures)
        .await
        .expect("first replay must apply cleanly");

    let account_count_1 = account::Entity::find().all(db.connection()).await.unwrap().len();
    let balance_count_1 = account_balance::Entity::find().all(db.connection()).await.unwrap().len();
    let tx_count_1 = transaction::Entity::find().all(db.connection()).await.unwrap().len();

    // Second replay of the identical corpus: nothing should change.
    let second = process_batch(db.connection(), GROUP, &registry, None, &fixtures)
        .await
        .expect("second replay must also apply cleanly");

    let account_count_2 = account::Entity::find().all(db.connection()).await.unwrap().len();
    let balance_count_2 = account_balance::Entity::find().all(db.connection()).await.unwrap().len();
    let tx_count_2 = transaction::Entity::find().all(db.connection()).await.unwrap().len();

    assert_eq!(account_count_1, account_count_2, "replay must not duplicate accounts");
    assert_eq!(balance_count_1, balance_count_2, "replay must not duplicate balances");
    assert_eq!(tx_count_1, tx_count_2, "replay must not duplicate transactions");
    assert_eq!(
        first.applied, second.applied,
        "an identical replay must apply the same number of records"
    );
    assert_eq!(
        first.skipped, second.skipped,
        "an identical replay must skip the same poison/unrecognized-topic records"
    );
    assert_eq!(
        first.skipped, 2,
        "the two source_account_id-less fixtures (§2.2 skip+log) must be \
         skipped; the two finreport.user-label fixtures are now recognized \
         and applied by projection::labeling (projector/mod.rs's \
         LABELING_PROJECTION_TOPICS wiring)"
    );

    // Reset (as a fresh DB would be) and replay once more: must reproduce
    // exactly the same row counts.
    let fresh = support::TestPostgres::start().await;
    let reset_replay = process_batch(fresh.connection(), GROUP, &registry, None, &fixtures)
        .await
        .expect("reset+replay must apply cleanly");
    let account_count_3 = account::Entity::find().all(fresh.connection()).await.unwrap().len();
    let balance_count_3 = account_balance::Entity::find().all(fresh.connection()).await.unwrap().len();
    let tx_count_3 = transaction::Entity::find().all(fresh.connection()).await.unwrap().len();

    assert_eq!(account_count_1, account_count_3);
    assert_eq!(balance_count_1, balance_count_3);
    assert_eq!(tx_count_1, tx_count_3);
    assert_eq!(first.applied, reset_replay.applied);
    assert_eq!(first.skipped, reset_replay.skipped);
}

#[tokio::test]
async fn source_precedence_wins_regardless_of_arrival_order() {
    let db = support::TestPostgres::start().await;
    let registry = MapperRegistry::with_default_mappers();

    let source_record = record(
        TRANSACTION_TOPIC,
        0,
        "ACC-PREC-1",
        transaction_payload("ACC-PREC-1", "100.00", "Source Holder"),
        Some("ACC-PREC"),
        "source",
    );
    let legacy_record = record(
        TRANSACTION_TOPIC,
        1,
        "ACC-PREC-1",
        legacy_transaction_payload("ACC-PREC-1", "ACC-PREC", "999.00"),
        Some("ACC-PREC"),
        "legacy-backfill",
    );

    // Order A: source arrives first, legacy-backfill arrives second — must
    // not overwrite the raw record.
    process_batch(db.connection(), GROUP, &registry, None, std::slice::from_ref(&source_record))
        .await
        .unwrap();
    process_batch(db.connection(), GROUP, &registry, None, std::slice::from_ref(&legacy_record))
        .await
        .unwrap();

    let row = transaction::Entity::find_by_id(webapp::kafka::envelope::transaction_uuid(
        "comdirect",
        "ACC-PREC-1",
    ))
    .one(db.connection())
    .await
    .unwrap()
    .expect("row must exist");
    assert_eq!(row.origin, "source", "source must win even arriving first");
    assert_eq!(row.amount, Decimal::from_str("100.00").unwrap());

    // Order B: legacy-backfill arrives first, source arrives second — source
    // must still win (it always wins, regardless of order).
    let db2 = support::TestPostgres::start().await;
    process_batch(db2.connection(), GROUP, &registry, None, std::slice::from_ref(&legacy_record))
        .await
        .unwrap();
    process_batch(db2.connection(), GROUP, &registry, None, std::slice::from_ref(&source_record))
        .await
        .unwrap();

    let row2 = transaction::Entity::find_by_id(webapp::kafka::envelope::transaction_uuid(
        "comdirect",
        "ACC-PREC-1",
    ))
    .one(db2.connection())
    .await
    .unwrap()
    .expect("row must exist");
    assert_eq!(row2.origin, "source", "source must win arriving second too");
    assert_eq!(row2.amount, Decimal::from_str("100.00").unwrap());
}

#[tokio::test]
async fn balance_precedence_also_prefers_source_regardless_of_order() {
    let db = support::TestPostgres::start().await;
    let registry = MapperRegistry::with_default_mappers();

    let source_balance = record(
        BALANCE_TOPIC,
        0,
        "ACC-BAL-1",
        balance_payload("500.00"),
        Some("ACC-BAL"),
        "source",
    );
    let mut legacy_envelope = envelope(Some("ACC-BAL"), "legacy-backfill");
    // A legacy balance observed the same calendar day as the source one, so
    // both rows target the same (account_id, balance_date) conflict key.
    legacy_envelope.imported_at = source_balance.envelope.imported_at;
    let legacy_balance = ConsumedRecord {
        topic: BALANCE_TOPIC.to_string(),
        partition: 0,
        offset: 1,
        key: Some("ACC-BAL-1".to_string()),
        payload: serde_json::to_vec(&balance_payload("1.00")).unwrap(),
        envelope: legacy_envelope,
    };

    process_batch(db.connection(), GROUP, &registry, None, std::slice::from_ref(&legacy_balance))
        .await
        .unwrap();
    process_batch(db.connection(), GROUP, &registry, None, std::slice::from_ref(&source_balance))
        .await
        .unwrap();

    let account_id = webapp::kafka::envelope::account_uuid("comdirect", "ACC-BAL");
    let balances = account_balance::Entity::find().all(db.connection()).await.unwrap();
    let row = balances
        .iter()
        .find(|b| b.account_id == account_id)
        .expect("balance row must exist");
    assert_eq!(row.origin, "source");
    assert_eq!(row.amount, Decimal::from_str("500.00").unwrap());
}

#[tokio::test]
async fn stub_account_is_filled_in_by_a_later_real_account_record() {
    let db = support::TestPostgres::start().await;
    let registry = MapperRegistry::with_default_mappers();

    let tx_record = record(
        TRANSACTION_TOPIC,
        0,
        "ACC-STUB-1",
        transaction_payload("ACC-STUB-1", "50.00", "Someone"),
        Some("ACC-STUB"),
        "source",
    );
    process_batch(db.connection(), GROUP, &registry, None, std::slice::from_ref(&tx_record))
        .await
        .unwrap();

    let account_id = webapp::kafka::envelope::account_uuid("comdirect", "ACC-STUB");
    let stub = account::Entity::find_by_id(account_id)
        .one(db.connection())
        .await
        .unwrap()
        .expect("stub account must exist");
    assert_eq!(stub.origin, "stub");

    let account_record = record(
        ACCOUNT_TOPIC,
        0,
        "ACC-STUB",
        account_payload("ACC-STUB"),
        None,
        "source",
    );
    process_batch(db.connection(), GROUP, &registry, None, std::slice::from_ref(&account_record))
        .await
        .unwrap();

    let filled_in = account::Entity::find_by_id(account_id)
        .one(db.connection())
        .await
        .unwrap()
        .expect("account must still exist under the same id");
    assert_eq!(
        filled_in.origin, "source",
        "a real account record must overwrite the stub"
    );
    assert_eq!(filled_in.iban.as_deref(), Some("DE11500105171234567890"));
}

#[tokio::test]
async fn first_writer_owns_the_account_link() {
    let db = support::TestPostgres::start().await;
    let registry = MapperRegistry::with_default_mappers();

    // Same (source, external_id) == same `reference`, claimed first by
    // ACC-A, then again (duplicate publish, e.g. seen through a second
    // login) by ACC-B. The row's account_id must stay ACC-A's.
    let first = record(
        TRANSACTION_TOPIC,
        0,
        "ACC-LINK-1",
        transaction_payload("ACC-LINK-1", "10.00", "Holder"),
        Some("ACC-A"),
        "source",
    );
    let second = record(
        TRANSACTION_TOPIC,
        1,
        "ACC-LINK-1",
        transaction_payload("ACC-LINK-1", "10.00", "Holder"),
        Some("ACC-B"),
        "source",
    );

    process_batch(db.connection(), GROUP, &registry, None, std::slice::from_ref(&first))
        .await
        .unwrap();
    process_batch(db.connection(), GROUP, &registry, None, std::slice::from_ref(&second))
        .await
        .unwrap();

    let row = transaction::Entity::find_by_id(webapp::kafka::envelope::transaction_uuid(
        "comdirect",
        "ACC-LINK-1",
    ))
    .one(db.connection())
    .await
    .unwrap()
    .expect("row must exist");

    assert_eq!(
        row.account_id,
        webapp::kafka::envelope::account_uuid("comdirect", "ACC-A"),
        "the account that claimed this reference first must keep the link"
    );
}

#[tokio::test]
async fn mid_batch_crash_leaves_no_duplicates_or_gaps() {
    let db = support::TestPostgres::start().await;
    let registry = MapperRegistry::with_default_mappers();
    let fixtures = support::load_fixture_records();

    // Simulate a process that applied the first half of the corpus, then
    // crashed before the second batch was ever attempted (a real kill before
    // a batch's transaction commits is strictly equivalent to "that batch
    // never happened", which batch atomicity already guarantees — the part
    // worth testing is that resuming from a different batch boundary still
    // converges to the same end state as one unbroken run).
    let midpoint = fixtures.len() / 2;
    let first_half = &fixtures[..midpoint];
    let second_half = &fixtures[midpoint..];

    process_batch(db.connection(), GROUP, &registry, None, first_half).await.unwrap();
    process_batch(db.connection(), GROUP, &registry, None, second_half).await.unwrap();

    let split_tx_count = transaction::Entity::find().all(db.connection()).await.unwrap().len();
    let split_account_count = account::Entity::find().all(db.connection()).await.unwrap().len();
    let split_balance_count = account_balance::Entity::find().all(db.connection()).await.unwrap().len();

    for topic in [ACCOUNT_TOPIC, BALANCE_TOPIC, TRANSACTION_TOPIC] {
        let expected_next_offset = fixtures
            .iter()
            .filter(|r| r.topic == topic)
            .map(|r| r.offset + 1)
            .max();
        if let Some(expected) = expected_next_offset {
            let stored = offset_for(db.connection(), GROUP, topic, 0).await.unwrap();
            assert_eq!(
                stored,
                Some(expected),
                "committed offset for {topic} must reach the last record's offset + 1"
            );
        }
    }

    // One unbroken run against a fresh DB must land on exactly the same
    // counts — no dups from the split, no gaps either.
    let unbroken = support::TestPostgres::start().await;
    process_batch(unbroken.connection(), GROUP, &registry, None, &fixtures).await.unwrap();
    let unbroken_tx_count = transaction::Entity::find().all(unbroken.connection()).await.unwrap().len();
    let unbroken_account_count = account::Entity::find().all(unbroken.connection()).await.unwrap().len();
    let unbroken_balance_count = account_balance::Entity::find()
        .all(unbroken.connection())
        .await
        .unwrap()
        .len();

    assert_eq!(split_tx_count, unbroken_tx_count);
    assert_eq!(split_account_count, unbroken_account_count);
    assert_eq!(split_balance_count, unbroken_balance_count);
}

#[tokio::test]
async fn default_owner_is_linked_when_configured() {
    use entity::entities::{app_user, user_account};
    use sea_orm::ActiveValue::Set;
    use sea_orm::ActiveModelTrait;

    let db = support::TestPostgres::start().await;
    let registry = MapperRegistry::with_default_mappers();

    let owner_id = Uuid::new_v4();
    app_user::ActiveModel {
        id: Set(owner_id),
        username: Set("test-owner".to_string()),
        password_hash: Set("unused".to_string()),
        display_name: Set(None),
        disabled: Set(false),
        created_at: Set(Utc::now().into()),
        is_admin: Set(false),
    }
    .insert(db.connection())
    .await
    .unwrap();

    let account_record = record(
        ACCOUNT_TOPIC,
        0,
        "ACC-OWNER-1",
        account_payload("ACC-OWNER-1"),
        None,
        "source",
    );
    process_batch(db.connection(), GROUP, &registry, Some(owner_id), std::slice::from_ref(&account_record))
        .await
        .unwrap();

    let link = user_account::Entity::find_by_id((
        owner_id,
        webapp::kafka::envelope::account_uuid("comdirect", "ACC-OWNER-1"),
    ))
    .one(db.connection())
    .await
    .unwrap();
    assert!(link.is_some(), "account must be linked to the configured default owner");
}

/// A manual `user-admin unlink` must stick: once the account row already
/// exists, re-processing a later record for it (replay, or just the next
/// normal batch) must not re-link the default owner. Covers the SHOULD-FIX
/// finding — `link_default_owner` used to run unconditionally on every
/// balance/transaction/account record, undoing the unlink on the very next
/// one.
#[tokio::test]
async fn unlinking_the_default_owner_is_not_undone_by_a_later_record() {
    use entity::entities::{app_user, user_account};
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, ModelTrait};

    let db = support::TestPostgres::start().await;
    let registry = MapperRegistry::with_default_mappers();

    let owner_id = Uuid::new_v4();
    app_user::ActiveModel {
        id: Set(owner_id),
        username: Set("test-owner-2".to_string()),
        password_hash: Set("unused".to_string()),
        display_name: Set(None),
        disabled: Set(false),
        created_at: Set(Utc::now().into()),
        is_admin: Set(false),
    }
    .insert(db.connection())
    .await
    .unwrap();

    let account_id = webapp::kafka::envelope::account_uuid("comdirect", "ACC-OWNER-2");

    // First record creates the account row and links the default owner.
    let account_record =
        record(ACCOUNT_TOPIC, 0, "ACC-OWNER-2", account_payload("ACC-OWNER-2"), None, "source");
    process_batch(db.connection(), GROUP, &registry, Some(owner_id), std::slice::from_ref(&account_record))
        .await
        .unwrap();

    let link = user_account::Entity::find_by_id((owner_id, account_id))
        .one(db.connection())
        .await
        .unwrap();
    assert!(link.is_some(), "the account-creating record must link the default owner");

    // Simulate `user-admin unlink`.
    link.unwrap().delete(db.connection()).await.unwrap();
    assert!(
        user_account::Entity::find_by_id((owner_id, account_id))
            .one(db.connection())
            .await
            .unwrap()
            .is_none(),
        "unlink must have removed the row"
    );

    // A later balance record for the same account (account row already
    // exists) must not re-link the owner.
    let balance_record = record(
        BALANCE_TOPIC,
        1,
        "bal-1",
        balance_payload("100.00"),
        Some("ACC-OWNER-2"),
        "source",
    );
    process_batch(db.connection(), GROUP, &registry, Some(owner_id), std::slice::from_ref(&balance_record))
        .await
        .unwrap();

    let relinked = user_account::Entity::find_by_id((owner_id, account_id))
        .one(db.connection())
        .await
        .unwrap();
    assert!(
        relinked.is_none(),
        "a manual unlink must not be undone by a later record for the same account"
    );
}

/// Offsets are scoped by group id: what group A committed is invisible to
/// group B, so B starts from nothing (== `Offset::Beginning`) and replaying
/// the same records under B rebuilds its offsets without disturbing A's.
#[tokio::test]
async fn offsets_are_scoped_by_group_so_a_new_group_replays_from_the_beginning() {
    let db = support::TestPostgres::start().await;
    let registry = MapperRegistry::with_default_mappers();
    let fixtures = support::load_fixture_records();

    process_batch(db.connection(), "group-a", &registry, None, &fixtures).await.unwrap();

    let a = load_offsets(db.connection(), "group-a").await.unwrap();
    assert!(!a.is_empty(), "group A committed offsets");
    assert!(
        load_offsets(db.connection(), "group-b").await.unwrap().is_empty(),
        "group B must see none of group A's offsets"
    );

    // Group B replays the whole log (idempotent upserts) and lands on the
    // same positions, while A's rows are untouched.
    process_batch(db.connection(), "group-b", &registry, None, &fixtures).await.unwrap();
    assert_eq!(load_offsets(db.connection(), "group-b").await.unwrap(), a);
    assert_eq!(load_offsets(db.connection(), "group-a").await.unwrap(), a);
}

/// The labeler's `@labeler`-suffixed keys and the group id are orthogonal:
/// the same topic key can hold independent positions in different groups.
#[tokio::test]
async fn labeler_suffixed_keys_are_scoped_by_group_too() {
    use webapp::projection::offsets::commit_offset;

    let db = support::TestPostgres::start().await;
    let key = "finreport.transaction@labeler";

    commit_offset(db.connection(), "group-a", key, 0, 10, Utc::now()).await.unwrap();
    commit_offset(db.connection(), "group-b", key, 0, 3, Utc::now()).await.unwrap();
    commit_offset(db.connection(), "group-a", key, 0, 12, Utc::now()).await.unwrap();

    let a = load_offsets(db.connection(), "group-a").await.unwrap();
    let b = load_offsets(db.connection(), "group-b").await.unwrap();
    assert_eq!(a.get(&(key.to_string(), 0)), Some(&12));
    assert_eq!(b.get(&(key.to_string(), 0)), Some(&3));
}
