//! The cash-account mutations end to end: a real schema, a real Kafka broker
//! and the shared Postgres. Gated behind the `integration` feature.
//!
//! The scoping tests are the point of this file: every mutation must refuse an
//! account that is not the caller's *cash* account, with one message that does
//! not reveal whether the id exists, and must never reach a bank account.
//!
//! Synthetic data only. Every external id is random per test, because all
//! tests share one database.
#![cfg(feature = "integration")]

mod common;
#[path = "support/mod.rs"]
mod support;

use async_graphql::{Request, Variables};
use chrono::{NaiveDate, Utc};
use entity::entities::{account, transaction, transaction_insight, user_account};
use rust_decimal::Decimal;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::{json, Value};
use std::sync::Arc;
use support::TestKafka;
use uuid::Uuid;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, AppSchema, RawSessionToken};
use webapp::kafka::cash::{TOPIC_CASH_ACCOUNT, TOPIC_CASH_TRANSACTION};
use webapp::kafka::envelope::{account_uuid, transaction_uuid};
use webapp::kafka::scan::scan_topic;
use webapp::projection::records::TransactionRecord;
use webapp::projection::upsert::{ensure_stub_account, upsert_transaction};

const CREATE_ACCOUNT: &str = "mutation($l: String!, $c: String!) { createCashAccount(label: $l, currency: $c) { id source label currency externalId isSavings } }";
const RENAME: &str = "mutation($id: UUID!, $l: String!) { renameCashAccount(accountId: $id, label: $l) { id label } }";
const DELETE_ACCOUNT: &str = "mutation($id: UUID!) { deleteCashAccount(accountId: $id) }";
const CREATE_TX: &str = "mutation($id: UUID!, $i: CashTransactionInput!) { createCashTransaction(accountId: $id, input: $i) { id accountId source amount currency description counterpartyName bookingDate bookingStatus } }";
const UPDATE_TX: &str = "mutation($id: UUID!, $i: CashTransactionInput!) { updateCashTransaction(transactionId: $id, input: $i) { id amount description counterpartyName bookingDate } }";
const DELETE_TX: &str = "mutation($id: UUID!) { deleteCashTransaction(transactionId: $id) }";

fn schema_for(db: Arc<DatabaseConnection>, kafka: Option<&TestKafka>) -> AppSchema {
    let mut settings = (*common::dummy_settings()).clone();
    settings.kafka_brokers = kafka.map(|k| k.bootstrap_servers().to_string());
    create_schema(db, Arc::new(settings))
}

async fn user(db: &DatabaseConnection) -> AuthenticatedUser {
    let (user_id, username) = common::seed_user(db, "cash-mut", "pw").await;
    AuthenticatedUser { user_id, username, account_ids: vec![] }
}

async fn run(
    schema: &AppSchema,
    who: &AuthenticatedUser,
    query: &str,
    vars: Value,
) -> async_graphql::Response {
    let request = Request::new(query).variables(Variables::from_json(vars));
    schema
        .execute(request_with_auth(request, Some(who.clone()), RawSessionToken::default()))
        .await
}

async fn ok(schema: &AppSchema, who: &AuthenticatedUser, query: &str, vars: Value) -> Value {
    let r = run(schema, who, query, vars).await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    r.data.into_json().unwrap()
}

fn code(r: &async_graphql::Response) -> String {
    r.errors[0].extensions.as_ref().unwrap().get("code").unwrap().to_string()
}

fn tx_input(amount: &str, description: Option<&str>) -> Value {
    json!({
        "bookingDate": "2026-03-04",
        "amount": amount,
        "currency": "EUR",
        "description": description,
        "counterpartyName": "Beispiel Baeckerei",
    })
}

async fn new_account(schema: &AppSchema, who: &AuthenticatedUser, label: &str) -> Uuid {
    let v = ok(schema, who, CREATE_ACCOUNT, json!({"l": label, "c": "EUR"})).await;
    v["createCashAccount"]["id"].as_str().unwrap().parse().unwrap()
}

async fn new_tx(schema: &AppSchema, who: &AuthenticatedUser, acct: Uuid, amount: &str) -> Uuid {
    let v = ok(schema, who, CREATE_TX, json!({"id": acct, "i": tx_input(amount, Some("Broetchen"))})).await;
    v["createCashTransaction"]["id"].as_str().unwrap().parse().unwrap()
}

fn topic_len(kafka: &TestKafka, topic: &str) -> usize {
    scan_topic(kafka.bootstrap_servers(), topic).unwrap().len()
}

/// A comdirect account the caller owns, with one transaction.
async fn bank_account_for(db: &DatabaseConnection, who: &AuthenticatedUser) -> (Uuid, Uuid, String) {
    let ext = format!("bank-{}", Uuid::new_v4());
    let tx_ext = format!("bank-tx-{}", Uuid::new_v4());
    let at = Utc::now();
    let id = ensure_stub_account(db, "comdirect", &ext, "EUR", at).await.unwrap();
    webapp::projection::upsert::link_default_owner(db, who.user_id, id, at).await.unwrap();
    upsert_transaction(
        db,
        &TransactionRecord {
            id: transaction_uuid("comdirect", &tx_ext),
            account_id: id,
            source: "comdirect".into(),
            external_id: tx_ext.clone(),
            booking_date: NaiveDate::from_ymd_opt(2026, 3, 4).unwrap(),
            valuta_date: None,
            booking_status: "BOOKED".into(),
            amount: Decimal::new(-5000, 2),
            currency: "EUR".into(),
            counterparty_name: None,
            counterparty_iban: None,
            description: Some("synthetic".into()),
            transaction_type: None,
            raw_payload: json!({}),
            origin: "source".into(),
            imported_at: at,
            updated_at: at,
            counterparty_key: None,
        },
    )
    .await
    .unwrap();
    (id, transaction_uuid("comdirect", &tx_ext), tx_ext)
}

#[tokio::test]
async fn account_and_transaction_lifecycle_publishes_and_projects() {
    let db = common::db().await;
    let kafka = TestKafka::start().await;
    let schema = schema_for(db.clone(), Some(&kafka));
    let me = user(&db).await;

    let created = ok(&schema, &me, CREATE_ACCOUNT, json!({"l": "  Wallet ", "c": "eur"})).await;
    let a = &created["createCashAccount"];
    assert_eq!((a["source"].as_str(), a["label"].as_str(), a["currency"].as_str()), (Some("cash"), Some("Wallet"), Some("EUR")));
    assert_eq!(a["isSavings"], false);
    let acct: Uuid = a["id"].as_str().unwrap().parse().unwrap();
    let ext = a["externalId"].as_str().unwrap().to_string();
    assert_eq!(acct, account_uuid("cash", &ext));
    assert!(Uuid::parse_str(&ext).is_ok(), "the external id is a generated UUID");
    assert!(!ext.contains("Wallet"));

    // Linked to the caller, owner named in the record.
    let links = user_account::Entity::find().filter(user_account::Column::AccountId.eq(acct)).all(&*db).await.unwrap();
    assert_eq!(links.iter().map(|l| l.user_id).collect::<Vec<_>>(), vec![me.user_id]);
    let rec = &scan_topic(kafka.bootstrap_servers(), TOPIC_CASH_ACCOUNT).unwrap()[0];
    assert_eq!((rec.header("source"), rec.header("origin")), (Some("cash"), Some("user")));
    assert_eq!(serde_json::from_slice::<Value>(rec.payload.as_ref().unwrap()).unwrap()["user_id"], me.user_id.to_string());

    // Create, then a transaction on it.
    let tx = ok(&schema, &me, CREATE_TX, json!({"id": acct, "i": tx_input("-4.50", Some("Broetchen"))})).await;
    let t = &tx["createCashTransaction"];
    assert_eq!((t["source"].as_str(), t["amount"].as_str(), t["bookingStatus"].as_str()), (Some("cash"), Some("-4.5000"), Some("BOOKED")));
    assert_eq!(t["accountId"], acct.to_string());
    let tx_id: Uuid = t["id"].as_str().unwrap().parse().unwrap();
    let published = scan_topic(kafka.bootstrap_servers(), TOPIC_CASH_TRANSACTION).unwrap();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].header("source_account_id"), Some(ext.as_str()));

    // Update replaces: the description is cleared by omission.
    let upd = ok(&schema, &me, UPDATE_TX, json!({"id": tx_id, "i": {
        "bookingDate": "2026-03-05", "amount": "-6.00", "currency": "EUR"}})).await;
    let u = &upd["updateCashTransaction"];
    assert_eq!((u["amount"].as_str(), u["bookingDate"].as_str()), (Some("-6.0000"), Some("2026-03-05")));
    assert!(u["description"].is_null() && u["counterpartyName"].is_null());
    assert_eq!(transaction::Entity::find().filter(transaction::Column::AccountId.eq(acct)).all(&*db).await.unwrap().len(), 1);

    // Rename.
    let renamed = ok(&schema, &me, RENAME, json!({"id": acct, "l": "Cash tin"})).await;
    assert_eq!(renamed["renameCashAccount"]["label"], "Cash tin");
    let stored = account::Entity::find_by_id(acct).one(&*db).await.unwrap().unwrap();
    assert_eq!(stored.label.as_deref(), Some("Cash tin"));
    assert_eq!(stored.raw_payload.unwrap()["user_id"], me.user_id.to_string(), "a rename keeps the owner");

    // Delete one transaction, add two, delete the account: nothing survives.
    ok(&schema, &me, DELETE_TX, json!({"id": tx_id})).await;
    assert!(transaction::Entity::find_by_id(tx_id).one(&*db).await.unwrap().is_none());
    let t1 = new_tx(&schema, &me, acct, "10.00").await;
    let t2 = new_tx(&schema, &me, acct, "-2.00").await;
    let before = topic_len(&kafka, TOPIC_CASH_TRANSACTION);
    let gone = ok(&schema, &me, DELETE_ACCOUNT, json!({"id": acct})).await;
    assert_eq!(gone["deleteCashAccount"], true);
    for id in [t1, t2] {
        assert!(transaction::Entity::find_by_id(id).one(&*db).await.unwrap().is_none());
    }
    assert!(account::Entity::find_by_id(acct).one(&*db).await.unwrap().is_none());
    assert!(user_account::Entity::find().filter(user_account::Column::AccountId.eq(acct)).all(&*db).await.unwrap().is_empty());

    // Both transactions were tombstoned on the compacted topic, then the account.
    let txs = scan_topic(kafka.bootstrap_servers(), TOPIC_CASH_TRANSACTION).unwrap();
    assert_eq!(txs.len(), before + 2);
    let tombstones = txs.iter().filter(|r| r.payload.is_none()).count();
    assert_eq!(tombstones, 3, "the deleted transaction plus the two cascaded");
    let accts = scan_topic(kafka.bootstrap_servers(), TOPIC_CASH_ACCOUNT).unwrap();
    assert!(accts.last().unwrap().payload.is_none());

    // The account is gone, so it is no longer found.
    let again = run(&schema, &me, DELETE_ACCOUNT, json!({"id": acct})).await;
    assert_eq!(code(&again), "\"NOT_FOUND\"");
}

#[tokio::test]
async fn two_users_with_the_same_label_get_different_accounts() {
    let db = common::db().await;
    let kafka = TestKafka::start().await;
    let schema = schema_for(db.clone(), Some(&kafka));
    let (a, b) = (user(&db).await, user(&db).await);
    let (x, y) = (new_account(&schema, &a, "Wallet").await, new_account(&schema, &b, "Wallet").await);
    assert_ne!(x, y);
}

#[tokio::test]
async fn another_users_cash_account_is_refused_identically_to_an_unknown_one() {
    let db = common::db().await;
    let kafka = TestKafka::start().await;
    let schema = schema_for(db.clone(), Some(&kafka));
    let (owner, intruder) = (user(&db).await, user(&db).await);
    let acct = new_account(&schema, &owner, "Wallet").await;
    let tx = new_tx(&schema, &owner, acct, "-4.50").await;
    let unknown = Uuid::new_v4();

    // An admin-style "link everything" must not confer ownership either.
    user_account::ActiveModel {
        user_id: Set(intruder.user_id),
        account_id: Set(acct),
        created_at: Set(Utc::now().into()),
    }
    .insert(&*db)
    .await
    .unwrap();

    let published = (topic_len(&kafka, TOPIC_CASH_ACCOUNT), topic_len(&kafka, TOPIC_CASH_TRANSACTION));
    for (who, id) in [(&intruder, acct), (&intruder, unknown)] {
        for (q, vars) in [
            (RENAME, json!({"id": id, "l": "Mine now"})),
            (DELETE_ACCOUNT, json!({"id": id})),
            (CREATE_TX, json!({"id": id, "i": tx_input("1.00", None)})),
        ] {
            let r = run(&schema, who, q, vars).await;
            assert_eq!(r.errors.len(), 1, "{q}");
            assert_eq!(code(&r), "\"NOT_FOUND\"");
            assert_eq!(r.errors[0].message, "cash account not found", "{q}");
        }
    }
    for id in [tx, unknown] {
        for (q, vars) in [
            (UPDATE_TX, json!({"id": id, "i": tx_input("1.00", None)})),
            (DELETE_TX, json!({"id": id})),
        ] {
            let r = run(&schema, &intruder, q, vars).await;
            assert_eq!(r.errors[0].message, "cash transaction not found", "{q}");
        }
    }

    // Nothing was published, nothing changed.
    assert_eq!((topic_len(&kafka, TOPIC_CASH_ACCOUNT), topic_len(&kafka, TOPIC_CASH_TRANSACTION)), published);
    let stored = account::Entity::find_by_id(acct).one(&*db).await.unwrap().unwrap();
    assert_eq!(stored.label.as_deref(), Some("Wallet"));
    assert_eq!(stored.raw_payload.unwrap()["user_id"], owner.user_id.to_string());
    assert!(transaction::Entity::find_by_id(tx).one(&*db).await.unwrap().is_some());
}

#[tokio::test]
async fn a_bank_account_is_refused_even_when_the_caller_owns_it() {
    let db = common::db().await;
    let kafka = TestKafka::start().await;
    let schema = schema_for(db.clone(), Some(&kafka));
    let me = user(&db).await;
    let (bank, bank_tx, _) = bank_account_for(&db, &me).await;

    for (q, vars) in [
        (RENAME, json!({"id": bank, "l": "Renamed"})),
        (DELETE_ACCOUNT, json!({"id": bank})),
        (CREATE_TX, json!({"id": bank, "i": tx_input("1.00", None)})),
    ] {
        let r = run(&schema, &me, q, vars).await;
        assert_eq!(code(&r), "\"NOT_FOUND\"", "{q}");
        assert_eq!(r.errors[0].message, "cash account not found", "{q}");
    }
    for (q, vars) in [
        (UPDATE_TX, json!({"id": bank_tx, "i": tx_input("1.00", None)})),
        (DELETE_TX, json!({"id": bank_tx})),
    ] {
        let r = run(&schema, &me, q, vars).await;
        assert_eq!(r.errors[0].message, "cash transaction not found", "{q}");
    }

    // The imported data is intact and nothing reached the cash topics.
    assert!(account::Entity::find_by_id(bank).one(&*db).await.unwrap().is_some());
    let tx = transaction::Entity::find_by_id(bank_tx).one(&*db).await.unwrap().unwrap();
    assert_eq!(tx.amount, Decimal::new(-5000, 2));
    assert_eq!(topic_len(&kafka, TOPIC_CASH_ACCOUNT), 0);
    assert_eq!(topic_len(&kafka, TOPIC_CASH_TRANSACTION), 0);
}

#[tokio::test]
async fn deleting_a_paired_cash_transaction_unpairs_the_bank_leg() {
    let db = common::db().await;
    let kafka = TestKafka::start().await;
    let schema = schema_for(db.clone(), Some(&kafka));
    let me = user(&db).await;
    let (_, bank_tx, _) = bank_account_for(&db, &me).await;
    let acct = new_account(&schema, &me, "Wallet").await;
    let cash_tx = new_tx(&schema, &me, acct, "50.00").await;

    // As the detector writes it: both legs flagged, pointing at each other.
    let now = Utc::now();
    for (id, other) in [(bank_tx, cash_tx), (cash_tx, bank_tx)] {
        transaction_insight::ActiveModel {
            transaction_id: Set(id),
            is_transfer: Set(true),
            transfer_counterpart_id: Set(Some(other)),
            transfer_match: Set(Some("amount_date".into())),
            is_recurring: Set(false),
            recurring_series_id: Set(None),
            recurring_cadence: Set(None),
            recurring_median_amount: Set(None),
            detected_at: Set(now.into()),
            revision: Set(now.into()),
        }
        .insert(&*db)
        .await
        .unwrap();
    }

    ok(&schema, &me, DELETE_TX, json!({"id": cash_tx})).await;

    let bank = transaction_insight::Entity::find_by_id(bank_tx).one(&*db).await.unwrap().unwrap();
    assert!(!bank.is_transfer && bank.transfer_counterpart_id.is_none() && bank.transfer_match.is_none());
    assert!(transaction_insight::Entity::find_by_id(cash_tx).one(&*db).await.unwrap().is_none());
}

#[tokio::test]
async fn invalid_input_is_refused_naming_a_field_and_never_a_value() {
    let db = common::db().await;
    let kafka = TestKafka::start().await;
    let schema = schema_for(db.clone(), Some(&kafka));
    let me = user(&db).await;
    let acct = new_account(&schema, &me, "Wallet").await;
    let before = topic_len(&kafka, TOPIC_CASH_TRANSACTION);

    let secret = "SECRET-PURPOSE-4711";
    let mut responses = Vec::new();
    // A mismatched currency, a zero and an over-precise amount, an over-long
    // description, a malformed currency and a blank label.
    for input in [
        json!({"bookingDate": "2026-03-04", "amount": "-4.50", "currency": "USD", "description": secret}),
        json!({"bookingDate": "2026-03-04", "amount": "0", "currency": "EUR", "description": secret}),
        json!({"bookingDate": "2026-03-04", "amount": "-4.123456", "currency": "EUR", "description": secret}),
        json!({"bookingDate": "2026-03-04", "amount": "-4.50", "currency": "EUR", "description": secret.repeat(100)}),
        json!({"bookingDate": "2026-03-04", "amount": "-4.50", "currency": secret}),
    ] {
        responses.push(run(&schema, &me, CREATE_TX, json!({"id": acct, "i": input})).await);
    }
    responses.push(run(&schema, &me, CREATE_ACCOUNT, json!({"l": "   ", "c": "EUR"})).await);
    responses.push(run(&schema, &me, CREATE_ACCOUNT, json!({"l": secret, "c": secret})).await);
    for r in &responses {
        assert!(!r.errors.is_empty());
        let text = format!("{:?}", r.errors);
        assert!(!text.contains("SECRET") && !text.contains("4.50") && !text.contains("4.123456"), "{text}");
    }
    assert_eq!(code(&responses[0]), "\"VALIDATION\"");
    assert_eq!(topic_len(&kafka, TOPIC_CASH_TRANSACTION), before, "a refused input publishes nothing");
}

#[tokio::test]
async fn without_a_broker_the_mutations_fail_cleanly() {
    let db = common::db().await;
    let schema = schema_for(db.clone(), None);
    let me = user(&db).await;
    let r = run(&schema, &me, CREATE_ACCOUNT, json!({"l": "Wallet", "c": "EUR"})).await;
    assert_eq!(code(&r), "\"KAFKA_UNAVAILABLE\"");
}
