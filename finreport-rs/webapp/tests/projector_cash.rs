//! Cash accounts through the projector: records land as `cash` rows linked to
//! the user the record names, replays change nothing, and a **tombstone**
//! removes the row without leaving a bank-side transfer pointing at it.
//!
//! The tombstone-and-pairing tests are the point of this file: the detector
//! flags both legs of a transfer, so deleting the cash leg must un-pair the
//! bank leg or the cashflow aggregations (which read `is_transfer`) silently
//! drop it.
//!
//! Synthetic data only. Gated behind the `integration` feature; uses the
//! shared throwaway-Postgres harness.
#![cfg(feature = "integration")]

#[path = "support/mod.rs"]
mod support;

use chrono::{NaiveDate, TimeZone, Utc};
use entity::entities::{
    account, app_user, transaction, transaction_insight, transaction_link, transaction_link_member,
    transaction_tag, user_account,
};
use rust_decimal::Decimal;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait, QueryOrder};
use uuid::Uuid;
use webapp::kafka::cash::{
    CashAccountRecord, CashTransactionRecord, CURRENT_SCHEMA_VERSION, SOURCE_CASH,
    TOPIC_CASH_ACCOUNT, TOPIC_CASH_TRANSACTION,
};
use webapp::kafka::envelope::{account_uuid, transaction_uuid, Envelope};
use webapp::projection::mapper::MapperRegistry;
use webapp::projection::records::TransactionRecord;
use webapp::projection::upsert::{ensure_stub_account, upsert_transaction};
use webapp::projection::{process_batch, ConsumedRecord};

const GROUP: &str = "default";
const USER: Uuid = Uuid::from_u128(0xAAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAA_AAAA);
const OTHER_USER: Uuid = Uuid::from_u128(0xBBBB_BBBB_BBBB_BBBB_BBBB_BBBB_BBBB_BBBB);
const WALLET: &str = "7a1c0000-0000-4000-8000-000000000001";
const TIN: &str = "7a1c0000-0000-4000-8000-000000000002";
const TX_ATM: &str = "7a1c0000-0000-4000-8000-0000000000a1";
const TX_BREAD: &str = "7a1c0000-0000-4000-8000-0000000000a2";
const BANK_ACCOUNT: &str = "bank-acct-1";
const BANK_TX_ATM: &str = "bank-tx-atm";
const BANK_TX_OTHER: &str = "bank-tx-other";

fn envelope(source_account_id: Option<&str>) -> Envelope {
    Envelope {
        source: SOURCE_CASH.to_string(),
        source_account_id: source_account_id.map(str::to_string),
        origin: "user".to_string(),
        schema_version: 1,
        imported_at: Utc.with_ymd_and_hms(2026, 3, 6, 9, 0, 0).unwrap(),
        comdirect_account_key: None,
        comdirect_account_name: None,
    }
}

/// What a tombstone looks like on the wire: no payload, and no headers, so the
/// envelope is the parser's default (which says `comdirect`).
fn tombstone_envelope() -> Envelope {
    Envelope {
        source: "comdirect".to_string(),
        origin: "source".to_string(),
        ..envelope(None)
    }
}

fn account_rec(offset: i64, external_id: &str, user: Uuid, label: &str) -> ConsumedRecord {
    let rec = CashAccountRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        external_id: external_id.to_string(),
        user_id: user,
        label: label.to_string(),
        currency: "EUR".to_string(),
    };
    ConsumedRecord {
        topic: TOPIC_CASH_ACCOUNT.to_string(),
        partition: 0,
        offset,
        key: Some(rec.topic_key().to_string()),
        payload: serde_json::to_vec(&rec).unwrap(),
        envelope: envelope(None),
    }
}

fn tx_rec(offset: i64, account: &str, external_id: &str, cents: i64) -> ConsumedRecord {
    let rec = CashTransactionRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        external_id: external_id.to_string(),
        account_external_id: account.to_string(),
        booking_date: NaiveDate::from_ymd_opt(2026, 3, 4).unwrap(),
        amount: Decimal::new(cents, 2),
        currency: "EUR".to_string(),
        counterparty_name: Some("Beispiel Baeckerei".to_string()),
        description: Some("Broetchen".to_string()),
    };
    ConsumedRecord {
        topic: TOPIC_CASH_TRANSACTION.to_string(),
        partition: 0,
        offset,
        key: Some(rec.topic_key().to_string()),
        payload: serde_json::to_vec(&rec).unwrap(),
        envelope: envelope(Some(account)),
    }
}

fn tombstone(offset: i64, topic: &str, key: &str) -> ConsumedRecord {
    ConsumedRecord {
        topic: topic.to_string(),
        partition: 0,
        offset,
        key: Some(key.to_string()),
        payload: Vec::new(),
        envelope: tombstone_envelope(),
    }
}

async fn project(db: &DatabaseConnection, default_owner: Option<Uuid>, batch: &[ConsumedRecord]) {
    let registry = MapperRegistry::with_default_mappers();
    process_batch(db, GROUP, &registry, default_owner, batch)
        .await
        .unwrap();
}

async fn add_user(db: &DatabaseConnection, id: Uuid, name: &str) {
    app_user::ActiveModel {
        id: Set(id),
        username: Set(name.to_string()),
        password_hash: Set("unused".to_string()),
        display_name: Set(None),
        disabled: Set(false),
        created_at: Set(Utc::now().into()),
        is_admin: Set(false),
    }
    .insert(db)
    .await
    .unwrap();
}

/// A bank-side (`comdirect`) account and two transactions: the ATM
/// withdrawal that funds the wallet, and an unrelated one.
async fn seed_bank(db: &DatabaseConnection) {
    let at = Utc.with_ymd_and_hms(2026, 3, 4, 8, 0, 0).unwrap();
    ensure_stub_account(db, "comdirect", BANK_ACCOUNT, "EUR", at)
        .await
        .unwrap();
    for (ext, cents) in [(BANK_TX_ATM, -5000), (BANK_TX_OTHER, -1200)] {
        upsert_transaction(
            db,
            &TransactionRecord {
                id: transaction_uuid("comdirect", ext),
                account_id: account_uuid("comdirect", BANK_ACCOUNT),
                source: "comdirect".to_string(),
                external_id: ext.to_string(),
                booking_date: NaiveDate::from_ymd_opt(2026, 3, 4).unwrap(),
                valuta_date: None,
                booking_status: "BOOKED".to_string(),
                amount: Decimal::new(cents, 2),
                currency: "EUR".to_string(),
                counterparty_name: None,
                counterparty_iban: None,
                description: Some("synthetic".to_string()),
                transaction_type: None,
                raw_payload: serde_json::json!({}),
                origin: "source".to_string(),
                imported_at: at,
                updated_at: at,
                counterparty_key: None,
            },
        )
        .await
        .unwrap();
    }
}

fn insight(
    id: Uuid,
    counterpart: Option<Uuid>,
    recurring: bool,
) -> transaction_insight::ActiveModel {
    let now = Utc::now();
    transaction_insight::ActiveModel {
        transaction_id: Set(id),
        is_transfer: Set(counterpart.is_some()),
        transfer_counterpart_id: Set(counterpart),
        transfer_match: Set(counterpart.map(|_| "amount_date".to_string())),
        is_recurring: Set(recurring),
        recurring_series_id: Set(recurring.then(Uuid::nil)),
        recurring_cadence: Set(recurring.then(|| "monthly".to_string())),
        recurring_median_amount: Set(recurring.then(|| Decimal::new(-5000, 2))),
        detected_at: Set(now.into()),
        revision: Set(now.into()),
    }
}

/// Pairs the bank ATM leg with `cash_tx`, both flagged, as the detector does.
/// The bank leg is also recurring, to prove only the transfer columns clear.
async fn pair_with_bank(db: &DatabaseConnection, cash_tx: &str) {
    let bank = transaction_uuid("comdirect", BANK_TX_ATM);
    let cash = transaction_uuid(SOURCE_CASH, cash_tx);
    insight(bank, Some(cash), true).insert(db).await.unwrap();
    insight(cash, Some(bank), false).insert(db).await.unwrap();
}

async fn bank_leg(db: &DatabaseConnection) -> transaction_insight::Model {
    transaction_insight::Entity::find_by_id(transaction_uuid("comdirect", BANK_TX_ATM))
        .one(db)
        .await
        .unwrap()
        .expect("the bank leg's insight row must survive")
}

async fn cash_tx_count(db: &DatabaseConnection) -> usize {
    transaction::Entity::find().all(db).await.unwrap().into_iter().filter(|t| t.source == "cash").count()
}

#[tokio::test]
async fn cash_records_land_linked_to_their_owner_and_replay_changes_nothing() {
    let pg = support::TestPostgres::start().await;
    let db = pg.connection();
    add_user(db, USER, "cash-user").await;
    add_user(db, OTHER_USER, "default-owner").await;
    let batch = vec![
        account_rec(0, WALLET, USER, "Wallet"),
        tx_rec(0, WALLET, TX_ATM, 5000),
        tx_rec(1, WALLET, TX_BREAD, -450),
    ];

    // A configured default owner must NOT be handed a user's wallet.
    project(db, Some(OTHER_USER), &batch).await;

    let accounts = account::Entity::find().all(db).await.unwrap();
    assert_eq!(accounts.len(), 1);
    let acct = &accounts[0];
    assert_eq!(acct.id, account_uuid("cash", WALLET));
    assert_eq!((acct.source.as_str(), acct.origin.as_str()), ("cash", "user"));
    assert_eq!(acct.label.as_deref(), Some("Wallet"));
    assert_eq!((acct.iban.as_deref(), acct.currency.as_str()), (None, "EUR"));

    let links = user_account::Entity::find().all(db).await.unwrap();
    assert_eq!(links.len(), 1, "only the record's own user, never the default owner");
    assert_eq!((links[0].user_id, links[0].account_id), (USER, acct.id));

    let txs = transaction::Entity::find()
        .order_by_asc(transaction::Column::Id)
        .all(db)
        .await
        .unwrap();
    assert_eq!(txs.len(), 2);
    assert!(txs.iter().all(|t| t.source == "cash" && t.origin == "user" && t.account_id == acct.id));
    let bread = txs.iter().find(|t| t.external_id == TX_BREAD).unwrap();
    assert_eq!(bread.id, transaction_uuid("cash", TX_BREAD));
    assert_eq!(bread.amount, Decimal::new(-450, 2));
    assert_eq!(bread.description.as_deref(), Some("Broetchen"));
    assert_eq!(bread.counterparty_key, None);

    // Replay: every row identical, still one link.
    project(db, Some(OTHER_USER), &batch).await;
    assert_eq!(
        transaction::Entity::find().order_by_asc(transaction::Column::Id).all(db).await.unwrap(),
        txs
    );
    assert_eq!(account::Entity::find().all(db).await.unwrap(), accounts);
    assert_eq!(user_account::Entity::find().all(db).await.unwrap(), links);

    // A re-published account record (a rename) updates in place.
    project(db, None, &[account_rec(2, WALLET, USER, "Holiday wallet")]).await;
    let renamed = account::Entity::find().one(db).await.unwrap().unwrap();
    assert_eq!(renamed.label.as_deref(), Some("Holiday wallet"));
    assert_eq!(renamed.first_seen_at, acct.first_seen_at);
}

#[tokio::test]
async fn an_unknown_owner_does_not_wedge_the_projector() {
    let pg = support::TestPostgres::start().await;
    let db = pg.connection();
    // No app_user row at all: `user_account.user_id` is a foreign key.
    project(db, None, &[account_rec(0, WALLET, USER, "Wallet")]).await;
    assert_eq!(account::Entity::find().all(db).await.unwrap().len(), 1);
    assert!(user_account::Entity::find().all(db).await.unwrap().is_empty());

    // Once the user exists, a replay of the same record links it.
    add_user(db, USER, "cash-user").await;
    project(db, None, &[account_rec(1, WALLET, USER, "Wallet")]).await;
    assert_eq!(user_account::Entity::find().all(db).await.unwrap().len(), 1);
}

#[tokio::test]
async fn deleting_a_paired_cash_transaction_unpairs_the_bank_leg() {
    let pg = support::TestPostgres::start().await;
    let db = pg.connection();
    add_user(db, USER, "cash-user").await;
    seed_bank(db).await;
    project(
        db,
        None,
        &[
            account_rec(0, WALLET, USER, "Wallet"),
            tx_rec(0, WALLET, TX_ATM, 5000),
            tx_rec(1, WALLET, TX_BREAD, -450),
        ],
    )
    .await;
    pair_with_bank(db, TX_ATM).await;
    let before = bank_leg(db).await;
    assert!(before.is_transfer);
    assert_eq!(before.transfer_counterpart_id, Some(transaction_uuid("cash", TX_ATM)));

    project(db, None, &[tombstone(2, TOPIC_CASH_TRANSACTION, TX_ATM)]).await;

    // The cash row, and the insight about it, are gone.
    assert_eq!(cash_tx_count(db).await, 1, "only the unrelated cash row survives");
    assert!(transaction::Entity::find_by_id(transaction_uuid("cash", TX_ATM))
        .one(db)
        .await
        .unwrap()
        .is_none());
    assert!(transaction_insight::Entity::find_by_id(transaction_uuid("cash", TX_ATM))
        .one(db)
        .await
        .unwrap()
        .is_none());

    // The bank leg is no longer a transfer and points at nothing - but its
    // recurring verdict, which has nothing to do with the cash row, stays.
    let after = bank_leg(db).await;
    assert!(!after.is_transfer);
    assert_eq!(after.transfer_counterpart_id, None);
    assert_eq!(after.transfer_match, None);
    assert!(after.is_recurring);
    assert_eq!(after.recurring_cadence.as_deref(), Some("monthly"));
    assert_eq!(after.revision, before.revision);
    // The bank transaction itself is untouched.
    assert!(transaction::Entity::find_by_id(transaction_uuid("comdirect", BANK_TX_ATM))
        .one(db)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn deleting_a_cash_account_does_the_same_for_every_transaction_it_owned() {
    let pg = support::TestPostgres::start().await;
    let db = pg.connection();
    add_user(db, USER, "cash-user").await;
    seed_bank(db).await;
    project(
        db,
        None,
        &[
            account_rec(0, WALLET, USER, "Wallet"),
            account_rec(1, TIN, USER, "Tin"),
            tx_rec(0, WALLET, TX_ATM, 5000),
            tx_rec(1, WALLET, TX_BREAD, -450),
            tx_rec(2, TIN, "7a1c0000-0000-4000-8000-0000000000b1", 100),
        ],
    )
    .await;
    pair_with_bank(db, TX_ATM).await;
    // A second pairing, to the other bank row, from the other wallet row.
    let other_bank = transaction_uuid("comdirect", BANK_TX_OTHER);
    let bread = transaction_uuid("cash", TX_BREAD);
    insight(other_bank, Some(bread), false).insert(db).await.unwrap();
    insight(bread, Some(other_bank), false).insert(db).await.unwrap();
    // Things projected about a wallet row: a tag, and a reimbursement link
    // that has the wallet row and a bank row as members.
    let now = Utc::now();
    transaction_tag::ActiveModel {
        transaction_id: Set(bread),
        tag: Set("lunch".to_string()),
        revision: Set(now.into()),
    }
    .insert(db)
    .await
    .unwrap();
    let link = Uuid::from_u128(7);
    transaction_link::ActiveModel {
        id: Set(link),
        kind: Set("reimbursement".to_string()),
        owner_user_id: Set(Some(USER)),
        note: Set(None),
        revision: Set(now.into()),
    }
    .insert(db)
    .await
    .unwrap();
    for (tx, role) in [(bread, "expense"), (other_bank, "offset")] {
        transaction_link_member::ActiveModel {
            link_id: Set(link),
            transaction_id: Set(tx),
            role: Set(role.to_string()),
        }
        .insert(db)
        .await
        .unwrap();
    }

    project(db, None, &[tombstone(3, TOPIC_CASH_ACCOUNT, WALLET)]).await;

    // The wallet, its rows and its owner link are gone; the tin is intact.
    let accounts = account::Entity::find().all(db).await.unwrap();
    let cash_ids: Vec<_> = accounts.iter().filter(|a| a.source == "cash").map(|a| a.external_id.as_str()).collect();
    assert_eq!(cash_ids, [TIN]);
    assert_eq!(cash_tx_count(db).await, 1);
    let links = user_account::Entity::find().all(db).await.unwrap();
    assert!(links.iter().all(|l| l.account_id != account_uuid("cash", WALLET)));
    assert_eq!(links.len(), 1);

    // Both bank legs are un-paired.
    for ext in [BANK_TX_ATM, BANK_TX_OTHER] {
        let row = transaction_insight::Entity::find_by_id(transaction_uuid("comdirect", ext))
            .one(db)
            .await
            .unwrap()
            .unwrap();
        assert!(!row.is_transfer, "{ext} still flagged as a transfer");
        assert_eq!(row.transfer_counterpart_id, None, "{ext} still points at a deleted row");
        assert_eq!(row.transfer_match, None);
    }
    assert_eq!(
        transaction_insight::Entity::find().all(db).await.unwrap().len(),
        2,
        "the wallet rows' own insight rows are gone"
    );

    // Their tag is gone, and the link that lost a member keeps the other one.
    assert!(transaction_tag::Entity::find().all(db).await.unwrap().is_empty());
    let members = transaction_link_member::Entity::find().all(db).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].transaction_id, other_bank);
}

#[tokio::test]
async fn a_link_left_with_no_members_is_removed() {
    let pg = support::TestPostgres::start().await;
    let db = pg.connection();
    add_user(db, USER, "cash-user").await;
    project(db, None, &[account_rec(0, WALLET, USER, "Wallet"), tx_rec(0, WALLET, TX_BREAD, -450)]).await;
    let bread = transaction_uuid("cash", TX_BREAD);
    let link = Uuid::from_u128(9);
    transaction_link::ActiveModel {
        id: Set(link),
        kind: Set("reimbursement".to_string()),
        owner_user_id: Set(Some(USER)),
        note: Set(None),
        revision: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .unwrap();
    transaction_link_member::ActiveModel {
        link_id: Set(link),
        transaction_id: Set(bread),
        role: Set("expense".to_string()),
    }
    .insert(db)
    .await
    .unwrap();

    project(db, None, &[tombstone(2, TOPIC_CASH_TRANSACTION, TX_BREAD)]).await;

    assert!(transaction_link::Entity::find().all(db).await.unwrap().is_empty());
    assert!(transaction_link_member::Entity::find().all(db).await.unwrap().is_empty());
}

#[tokio::test]
async fn tombstones_for_unknown_keys_are_harmless_and_replay_safe() {
    let pg = support::TestPostgres::start().await;
    let db = pg.connection();
    add_user(db, USER, "cash-user").await;
    project(
        db,
        None,
        &[
            tombstone(0, TOPIC_CASH_TRANSACTION, "never-existed"),
            tombstone(1, TOPIC_CASH_ACCOUNT, "never-existed"),
        ],
    )
    .await;

    // Delete, then replay the whole log (record, tombstone) over a database
    // that already applied it: the end state is the same.
    let log = [
        account_rec(2, WALLET, USER, "Wallet"),
        tx_rec(3, WALLET, TX_BREAD, -450),
        tombstone(4, TOPIC_CASH_TRANSACTION, TX_BREAD),
    ];
    project(db, None, &log).await;
    project(db, None, &log).await;
    assert_eq!(cash_tx_count(db).await, 0);
    assert_eq!(account::Entity::find().all(db).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_malformed_cash_record_is_skipped_not_fatal() {
    let pg = support::TestPostgres::start().await;
    let db = pg.connection();
    let registry = MapperRegistry::with_default_mappers();
    let mut bad = tx_rec(0, WALLET, TX_BREAD, -450);
    bad.payload = br#"{"amount":"SECRET-AMOUNT"}"#.to_vec();
    let stats = process_batch(db, GROUP, &registry, None, &[bad]).await.unwrap();
    assert_eq!((stats.applied, stats.skipped), (0, 1));
    assert_eq!(cash_tx_count(db).await, 0);
}
