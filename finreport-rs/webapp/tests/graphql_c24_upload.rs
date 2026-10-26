//! `uploadC24Transactions` end to end: a real schema, a real Kafka broker and
//! the shared Postgres. Gated behind the `integration` feature.
//!
//! Synthetic data only. Every account number is random per test, because all
//! tests share one database.
#![cfg(feature = "integration")]

mod common;
#[path = "support/mod.rs"]
mod support;

use async_graphql::Request;
use entity::entities::{account, transaction, user_account};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;
use std::sync::Arc;
use support::TestKafka;
use uuid::Uuid;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, AppSchema, RawSessionToken};
use webapp::kafka::c24::{SOURCE_C24, TOPIC_C24_ACCOUNT, TOPIC_C24_TRANSACTION};
use webapp::kafka::envelope::{account_uuid, transaction_uuid};
use webapp::kafka::scan::scan_topic;
use webapp::projection::c24::C24Mapper;
use webapp::projection::mapper::{MapperInput, SourceMapper};

const HEADER: &str = "Transaktionstyp,Buchungsdatum,Karteneinsatz,Betrag,Zahlungsempfänger,IBAN,BIC,Verwendungszweck,Beschreibung,Kontonummer,Kontoname,Kategorie,Unterkategorie,Bargeldabhebung";

const MUTATION: &str = "mutation($files: [String!]!) { uploadC24Transactions(files: $files) { \
    newTransactions unchangedTransactions updatedTransactions newAccounts updatedAccounts accountCount \
    earliestBookingDate latestBookingDate files { index rowCount error } } }";

fn account_number() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

fn card(acct: &str, day: &str, category: &str) -> String {
    format!(r#"Kartenzahlung,{day},03.03.2026 18:42,"-1,00 €",Beispiel Baeckerei,,,,,{acct},Testkonto,{category},Baeckerei"#)
}

fn transfer(acct: &str) -> String {
    format!(r#"Echtzeit-Überweisung,05.03.2026,,"1.500,00 €",Max Mustermann,DE00000000000000000000,TESTDE00XXX,Miete Maerz,,{acct},Testkonto,,"#)
}

fn file(rows: &[String]) -> String {
    let mut s = HEADER.to_string();
    for r in rows {
        s.push('\n');
        s.push_str(r);
    }
    s
}

async fn caller(db: &DatabaseConnection) -> AuthenticatedUser {
    let (user_id, username) = common::seed_user(db, "c24-up", "pw").await;
    AuthenticatedUser { user_id, username, account_ids: vec![] }
}

fn schema_for(db: Arc<DatabaseConnection>, kafka: Option<&TestKafka>) -> AppSchema {
    let mut settings = (*common::dummy_settings()).clone();
    settings.kafka_brokers = kafka.map(|k| k.bootstrap_servers().to_string());
    create_schema(db, Arc::new(settings))
}

async fn upload(schema: &AppSchema, user: &AuthenticatedUser, files: Vec<String>) -> async_graphql::Response {
    let request = Request::new(MUTATION).variables(async_graphql::Variables::from_json(serde_json::json!({ "files": files })));
    schema.execute(request_with_auth(request, Some(user.clone()), RawSessionToken::default())).await
}

async fn upload_ok(schema: &AppSchema, user: &AuthenticatedUser, files: Vec<String>) -> Value {
    let r = upload(schema, user, files).await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    r.data.into_json().unwrap()["uploadC24Transactions"].clone()
}

fn counts(v: &Value) -> [i64; 5] {
    ["newTransactions", "unchangedTransactions", "updatedTransactions", "newAccounts", "updatedAccounts"]
        .map(|k| v[k].as_i64().unwrap())
}

async fn txs_of(db: &DatabaseConnection, acct: &str) -> Vec<transaction::Model> {
    transaction::Entity::find()
        .filter(transaction::Column::AccountId.eq(account_uuid(SOURCE_C24, acct)))
        .all(db)
        .await
        .unwrap()
}

fn topic_len(kafka: &TestKafka, topic: &str) -> usize {
    scan_topic(kafka.bootstrap_servers(), topic).unwrap().len()
}

#[tokio::test]
async fn upload_projects_links_and_is_idempotent_across_overlapping_exports() {
    let db = common::db().await;
    let kafka = TestKafka::start().await;
    let schema = schema_for(db.clone(), Some(&kafka));
    let user = caller(&db).await;
    let acct = account_number();
    let acct_id = account_uuid(SOURCE_C24, &acct);

    // Two byte-identical card rows (distinct transactions) plus a transfer.
    let narrow = file(&[card(&acct, "04.03.2026", "Essen"), card(&acct, "04.03.2026", "Essen"), transfer(&acct)]);
    let first = upload_ok(&schema, &user, vec![narrow.clone()]).await;
    assert_eq!(counts(&first), [3, 0, 0, 1, 0]);
    assert_eq!(first["accountCount"], 1);
    assert_eq!(first["earliestBookingDate"], "2026-03-04");
    assert_eq!(first["latestBookingDate"], "2026-03-05");

    // Projected under c24, and the account is linked to the caller.
    let stored = txs_of(&db, &acct).await;
    assert_eq!(stored.len(), 3);
    assert!(stored.iter().all(|t| t.source == SOURCE_C24));
    let links = user_account::Entity::find()
        .filter(user_account::Column::UserId.eq(user.user_id))
        .all(&*db)
        .await
        .unwrap();
    assert_eq!(links.iter().map(|l| l.account_id).collect::<Vec<_>>(), vec![acct_id]);

    // A published transaction record carries what the mapper needs on replay:
    // `source_account_id` off the envelope. This is what a two-header publish
    // would have silently broken.
    let published = scan_topic(kafka.bootstrap_servers(), TOPIC_C24_TRANSACTION).unwrap();
    assert_eq!(published.len(), 3);
    for rec in &published {
        assert_eq!(rec.header("source"), Some(SOURCE_C24));
        assert_eq!(rec.header("origin"), Some("source"));
        assert!(rec.header("comdirect_account_key").is_none());
        let payload = rec.payload.clone().unwrap();
        let key = rec.key.clone().unwrap();
        let mapped = C24Mapper
            .map_transaction(&MapperInput {
                event: webapp::kafka::envelope::SourceEvent {
                    source: rec.header("source").unwrap(),
                    source_account_id: rec.header("source_account_id"),
                    key: &key,
                    payload: &payload,
                    imported_at: chrono::Utc::now(),
                },
                comdirect_account_key: None,
                comdirect_account_name: None,
            })
            .expect("a published record must project on replay");
        assert_eq!(mapped.account_id, acct_id);
        assert_eq!(mapped.id, transaction_uuid(SOURCE_C24, &key));
        assert!(stored.iter().any(|t| t.id == mapped.id));
    }
    assert_eq!(topic_len(&kafka, TOPIC_C24_ACCOUNT), 1);

    // The same file again: nothing published, nothing changed.
    let before = txs_of(&db, &acct).await;
    let again = upload_ok(&schema, &user, vec![narrow]).await;
    assert_eq!(counts(&again), [0, 3, 0, 0, 0]);
    assert_eq!(topic_len(&kafka, TOPIC_C24_TRANSACTION), 3);
    assert_eq!(topic_len(&kafka, TOPIC_C24_ACCOUNT), 1);
    let mut after = txs_of(&db, &acct).await;
    let mut before = before;
    before.sort_by_key(|t| t.id);
    after.sort_by_key(|t| t.id);
    assert_eq!(before, after);
    assert_eq!(
        user_account::Entity::find().filter(user_account::Column::UserId.eq(user.user_id)).all(&*db).await.unwrap().len(),
        1,
        "linking is idempotent"
    );

    // A wider, overlapping export: only the genuinely new row is added.
    let wide = file(&[
        card(&acct, "04.03.2026", "Essen"),
        card(&acct, "04.03.2026", "Essen"),
        transfer(&acct),
        card(&acct, "09.03.2026", "Essen"),
    ]);
    let wider = upload_ok(&schema, &user, vec![wide]).await;
    assert_eq!(counts(&wider), [1, 3, 0, 0, 0]);
    assert_eq!(txs_of(&db, &acct).await.len(), 4);
    assert_eq!(topic_len(&kafka, TOPIC_C24_TRANSACTION), 4);

    // C24 re-categorising a row keeps its id and counts as an update.
    let recategorised = file(&[card(&acct, "09.03.2026", "Freizeit")]);
    let updated = upload_ok(&schema, &user, vec![recategorised]).await;
    assert_eq!(counts(&updated), [0, 0, 1, 0, 0]);
    assert_eq!(txs_of(&db, &acct).await.len(), 4);
    let moved = txs_of(&db, &acct)
        .await
        .into_iter()
        .find(|t| t.raw_payload["c24_category"] == "Freizeit")
        .expect("the updated payload is stored");
    assert_eq!(moved.booking_date.to_string(), "2026-03-09");
}

#[tokio::test]
async fn a_bad_file_is_rejected_by_position_and_never_echoes_a_cell() {
    let db = common::db().await;
    let kafka = TestKafka::start().await;
    let schema = schema_for(db.clone(), Some(&kafka));
    let user = caller(&db).await;
    let acct = account_number();

    let secret = "SECRETPAYEE-NOT-AN-AMOUNT";
    let bad_row = format!(r#"Kartenzahlung,04.03.2026,,"{secret}",Beispiel Baeckerei,,,,,{acct},Testkonto,,"#);
    let good = file(&[transfer(&acct)]);
    let r = upload(&schema, &user, vec![good, file(&[bad_row]), "garbage".to_string()]).await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let text = serde_json::to_string(&r.data.into_json().unwrap()).unwrap();
    assert!(!text.contains(secret), "{text}");
    assert!(!text.contains(&acct), "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    let v = &v["uploadC24Transactions"];
    assert_eq!(v["files"][0]["error"], Value::Null);
    assert!(v["files"][1]["error"].as_str().unwrap().contains("Betrag"));
    assert_eq!(v["files"][1]["index"], 1);
    assert!(v["files"][2]["error"].is_string());
    // The good file still imported; the bad one contributed nothing.
    assert_eq!(v["newTransactions"], 1);
    assert_eq!(txs_of(&db, &acct).await.len(), 1);
}

#[tokio::test]
async fn an_account_owned_by_someone_else_is_refused_and_not_linked() {
    let db = common::db().await;
    let kafka = TestKafka::start().await;
    let schema = schema_for(db.clone(), Some(&kafka));
    let owner = caller(&db).await;
    let intruder = caller(&db).await;
    let acct = account_number();
    let f = file(&[transfer(&acct)]);

    upload_ok(&schema, &owner, vec![f.clone()]).await;

    let r = upload(&schema, &intruder, vec![f.clone()]).await;
    assert_eq!(r.errors.len(), 1);
    assert!(!r.errors[0].message.contains(&acct));
    let links = user_account::Entity::find()
        .filter(user_account::Column::AccountId.eq(account_uuid(SOURCE_C24, &acct)))
        .all(&*db)
        .await
        .unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].user_id, owner.user_id);

    // The owner is unaffected and may re-upload.
    let again = upload_ok(&schema, &owner, vec![f]).await;
    assert_eq!(counts(&again), [0, 1, 0, 0, 0]);
    assert_eq!(account::Entity::find_by_id(account_uuid(SOURCE_C24, &acct)).all(&*db).await.unwrap().len(), 1);
}

#[tokio::test]
async fn without_a_broker_the_mutation_fails_cleanly() {
    let db = common::db().await;
    let schema = schema_for(db.clone(), None);
    let user = caller(&db).await;
    let r = upload(&schema, &user, vec![file(&[transfer(&account_number())])]).await;
    assert_eq!(r.errors.len(), 1);
    assert_eq!(r.errors[0].extensions.as_ref().unwrap().get("code").unwrap().to_string(), "\"KAFKA_UNAVAILABLE\"");
}
