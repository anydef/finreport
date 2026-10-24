//! `counterpartyBreakdown`: signed net per counterparty, sorted by magnitude,
//! a tail `other` row, a single no-key bucket, scoped to the caller.
#![cfg(feature = "integration")]

mod common;

use async_graphql::{Request, Variables};
use entity::entities::transaction;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, RawSessionToken};

const QUERY: &str = "query($f: TransactionFilter!, $l: Int!) { counterpartyBreakdown(filter: $f, limit: $l) { \
    rows { counterpartyKey displayName amount transactionCount } \
    other { counterpartyKey displayName amount transactionCount } currency } }";

async fn user_with_account(db: &DatabaseConnection, name: &str) -> (AuthenticatedUser, Uuid) {
    let (user_id, username) = common::seed_user(db, name, "pw").await;
    let account = common::seed_account(db, "EUR", name).await;
    common::link(db, user_id, account).await;
    (AuthenticatedUser { user_id, username, account_ids: vec![account] }, account)
}

/// Seeds a transaction and sets its counterparty key (the shared helper
/// leaves it NULL).
async fn tx(db: &DatabaseConnection, account: Uuid, amount: &str, key: Option<&str>, name: &str) {
    let id = common::seed_transaction(db, account, "2024-03-01", amount, Some(name)).await;
    let mut m: transaction::ActiveModel = transaction::Entity::find_by_id(id).one(db).await.unwrap().unwrap().into();
    m.counterparty_key = Set(key.map(str::to_string));
    m.update(db).await.unwrap();
}

async fn run(
    schema: &webapp::graphql::AppSchema,
    user: &AuthenticatedUser,
    filter: serde_json::Value,
    limit: i32,
) -> serde_json::Value {
    let req = Request::new(QUERY).variables(Variables::from_json(serde_json::json!({ "f": filter, "l": limit })));
    let response = schema.execute(request_with_auth(req, Some(user.clone()), RawSessionToken::default())).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()["counterpartyBreakdown"].clone()
}

fn amt(v: &serde_json::Value) -> f64 {
    v.as_str().unwrap().parse().unwrap()
}

fn keys(b: &serde_json::Value) -> Vec<Option<String>> {
    b["rows"].as_array().unwrap().iter().map(|r| r["counterpartyKey"].as_str().map(String::from)).collect()
}

#[tokio::test]
async fn sorted_by_magnitude_with_signed_nets_and_refund_netting() {
    let db = common::db().await;
    let (user, account) = user_with_account(&db, "cpb-sort").await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    tx(&db, account, "-50.00", Some("shop"), "Shop").await;
    tx(&db, account, "20.00", Some("shop"), "Shop").await; // refund: nets to -30
    tx(&db, account, "1000.00", Some("employer"), "Employer").await;
    tx(&db, account, "-5.00", Some("kiosk"), "Kiosk").await;

    let b = run(&schema, &user, serde_json::json!({}), 20).await;
    assert_eq!(keys(&b), vec![Some("employer".into()), Some("shop".into()), Some("kiosk".into())]);
    let rows = b["rows"].as_array().unwrap();
    assert_eq!(amt(&rows[0]["amount"]), 1000.0);
    assert_eq!(amt(&rows[1]["amount"]), -30.0);
    assert_eq!(rows[1]["transactionCount"], 2);
    assert_eq!(rows[1]["displayName"], "Shop");
    assert_eq!(amt(&rows[2]["amount"]), -5.0);
    assert!(b["other"].is_null());
    assert_eq!(b["currency"], "EUR");
}

#[tokio::test]
async fn keyless_transactions_form_one_bucket() {
    let db = common::db().await;
    let (user, account) = user_with_account(&db, "cpb-null").await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    tx(&db, account, "-10.00", None, "A").await;
    tx(&db, account, "-20.00", Some(""), "B").await; // empty key counts as no key
    tx(&db, account, "-1.00", Some("k"), "K").await;

    let b = run(&schema, &user, serde_json::json!({}), 20).await;
    assert_eq!(keys(&b), vec![None, Some("k".into())]);
    let first = &b["rows"][0];
    assert_eq!(amt(&first["amount"]), -30.0);
    assert_eq!(first["transactionCount"], 2);
    assert_eq!(first["displayName"], "No merchant key");
}

#[tokio::test]
async fn limit_puts_the_rest_in_other_and_the_total_still_adds_up() {
    let db = common::db().await;
    let (user, account) = user_with_account(&db, "cpb-limit").await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    tx(&db, account, "-100.00", Some("a"), "A").await;
    tx(&db, account, "-40.00", Some("b"), "B").await;
    tx(&db, account, "30.00", Some("c"), "C").await;
    tx(&db, account, "-5.00", Some("d"), "D").await;
    tx(&db, account, "-5.00", Some("d"), "D").await;

    let b = run(&schema, &user, serde_json::json!({}), 2).await;
    assert_eq!(keys(&b), vec![Some("a".into()), Some("b".into())]);
    let other = &b["other"];
    assert!(other["counterpartyKey"].is_null());
    assert_eq!(other["displayName"], "Other");
    assert_eq!(other["transactionCount"], 3);
    assert_eq!(amt(&other["amount"]), 20.0);
    let rows_sum: f64 = b["rows"].as_array().unwrap().iter().map(|r| amt(&r["amount"])).sum();
    assert_eq!(rows_sum + amt(&other["amount"]), -100.0 - 40.0 + 30.0 - 10.0);

    // Exactly as many as distinct counterparties: nothing beyond, `other` null.
    let b = run(&schema, &user, serde_json::json!({}), 4).await;
    assert_eq!(b["rows"].as_array().unwrap().len(), 4);
    assert!(b["other"].is_null());
}

#[tokio::test]
async fn other_users_transactions_never_appear() {
    let db = common::db().await;
    let (user, account) = user_with_account(&db, "cpb-mine").await;
    let (_o, other_account) = user_with_account(&db, "cpb-theirs").await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    tx(&db, account, "-7.00", Some("mine"), "Mine").await;
    tx(&db, other_account, "-999.00", Some("theirs"), "Theirs").await;

    let b = run(&schema, &user, serde_json::json!({}), 20).await;
    assert_eq!(keys(&b), vec![Some("mine".into())]);
    assert!(b["other"].is_null());
}

#[tokio::test]
async fn an_empty_result_is_empty_not_an_error() {
    let db = common::db().await;
    let (user, account) = user_with_account(&db, "cpb-empty").await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    tx(&db, account, "-7.00", Some("k"), "K").await;

    let b = run(&schema, &user, serde_json::json!({ "startDate": "2030-01-01", "endDate": "2030-12-31" }), 20).await;
    assert!(b["rows"].as_array().unwrap().is_empty());
    assert!(b["other"].is_null());
}
