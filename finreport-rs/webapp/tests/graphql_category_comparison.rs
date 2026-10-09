//! `categoryComparison`: per-category and total figures across periods, a
//! refund netting inside its period, transfers and held rows kept out of the
//! total, and the caller's account scoping.
#![cfg(feature = "integration")]

mod common;

use async_graphql::Request;
use rust_decimal::Decimal;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use std::str::FromStr;
use uuid::Uuid;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, RawSessionToken};

fn dec(v: &serde_json::Value) -> Decimal {
    Decimal::from_str(v.as_str().unwrap()).unwrap()
}

async fn mark_transfer(db: &DatabaseConnection, tx: Uuid) {
    db.execute(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO transaction_insight (transaction_id, is_transfer, is_recurring, detected_at, revision) \
         VALUES ($1, true, false, now(), now())",
        vec![tx.into()],
    ))
    .await
    .unwrap();
}

async fn user_with_account(db: &DatabaseConnection, name: &str) -> (AuthenticatedUser, Uuid) {
    let (user_id, username) = common::seed_user(db, name, "pw").await;
    let account = common::seed_account(db, "EUR", name).await;
    common::link(db, user_id, account).await;
    (AuthenticatedUser { user_id, username, account_ids: vec![account] }, account)
}

async fn run(
    schema: &webapp::graphql::AppSchema,
    user: &AuthenticatedUser,
    args: &str,
) -> serde_json::Value {
    let q = format!(
        "{{ categoryComparison({args}) {{ currency \
           periods {{ start end total uncategorized needsReview }} \
           categories {{ category {{ slug }} total cells {{ amount transactionCount }} }} }} }}"
    );
    let response = schema
        .execute(request_with_auth(Request::new(q), Some(user.clone()), RawSessionToken::default()))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()["categoryComparison"].clone()
}

fn cat<'a>(out: &'a serde_json::Value, prefix: &str) -> &'a serde_json::Value {
    out["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["category"]["slug"].as_str().unwrap().starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix} category in {out}"))
}

fn amounts(c: &serde_json::Value) -> Vec<Decimal> {
    c["cells"].as_array().unwrap().iter().map(|cell| dec(&cell["amount"])).collect()
}

fn dec_s(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[tokio::test]
async fn figures_per_category_and_in_total_across_months() {
    let db = common::db().await;
    let (user, account) = user_with_account(&db, "cmp-main").await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    let (food, _) = common::seed_category(&db, "cmpfood", "Food", "expense").await;
    let (gym, _) = common::seed_category(&db, "cmpgym", "Gym", "expense").await;
    let (travel, _) = common::seed_category(&db, "cmptravel", "Travel", "expense").await;
    let (health, _) = common::seed_category(&db, "cmphealth", "Health", "expense").await;

    let tx = |date: &'static str, amount: &'static str, cat: Option<Uuid>, status: &'static str| {
        let db = db.clone();
        async move {
            let t = common::seed_transaction(&db, account, date, amount, Some("Shop")).await;
            if let Some(cat) = cat {
                common::seed_transaction_label(&db, t, Some(cat), "rule", status).await;
            }
            t
        }
    };

    // August: food 100 (two transactions), gym 30.
    tx("2026-08-03", "-60.00", Some(food), "applied").await;
    tx("2026-08-20", "-40.00", Some(food), "applied").await;
    tx("2026-08-15", "-30.00", Some(gym), "applied").await;
    // September: food 120, travel 400 (new), gym gone, health bill reimbursed
    // in full (nets to 0, not 2000), an unlabelled 25, a held 70.
    tx("2026-09-05", "-120.00", Some(food), "applied").await;
    tx("2026-09-12", "-400.00", Some(travel), "applied").await;
    tx("2026-09-10", "-1000.00", Some(health), "applied").await;
    tx("2026-09-20", "1000.00", Some(health), "applied").await;
    tx("2026-09-22", "-25.00", None, "applied").await;
    tx("2026-09-25", "-70.00", Some(food), "needs_review").await;
    // A transfer labelled food must not count anywhere.
    let transfer = tx("2026-09-26", "-500.00", Some(food), "applied").await;
    mark_transfer(&db, transfer).await;

    let out = run(&schema, &user, r#"filter: { startDate: "2026-08-01", endDate: "2026-09-30" }"#).await;

    let periods = out["periods"].as_array().unwrap();
    assert_eq!(periods.len(), 2);
    assert_eq!((periods[0]["start"].as_str(), periods[0]["end"].as_str()), (Some("2026-08-01"), Some("2026-08-31")));
    assert_eq!((periods[1]["start"].as_str(), periods[1]["end"].as_str()), (Some("2026-09-01"), Some("2026-09-30")));

    assert_eq!(amounts(cat(&out, "cmpfood")), [dec_s("100"), dec_s("120")]);
    assert_eq!(amounts(cat(&out, "cmpgym")), [dec_s("30"), dec_s("0")]);
    assert_eq!(amounts(cat(&out, "cmptravel")), [dec_s("0"), dec_s("400")]);
    assert_eq!(dec(&cat(&out, "cmpfood")["total"]), dec_s("220"));

    // The refund cancels the bill rather than doubling it, yet both count.
    let h = cat(&out, "cmphealth");
    assert_eq!(amounts(h), [dec_s("0"), dec_s("0")]);
    assert_eq!(h["cells"][1]["transactionCount"], 2);
    assert_eq!(h["cells"][0]["transactionCount"], 0);

    // Totals: categories + uncategorised; held (70) and the transfer (500) out.
    assert_eq!(dec(&periods[0]["total"]), dec_s("130"));
    assert_eq!(dec(&periods[1]["total"]), dec_s("120") + dec_s("400") + dec_s("25"));
    assert_eq!(dec(&periods[1]["uncategorized"]), dec_s("25"));
    assert_eq!(dec(&periods[1]["needsReview"]), dec_s("70"));
    assert_eq!(dec(&periods[0]["needsReview"]), dec_s("0"));
}

#[tokio::test]
async fn only_the_callers_accounts_are_compared() {
    let db = common::db().await;
    let (user, account) = user_with_account(&db, "cmp-mine").await;
    let (_other, other_account) = user_with_account(&db, "cmp-theirs").await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    let (food, _) = common::seed_category(&db, "cmpscope", "Food", "expense").await;

    let mine = common::seed_transaction(&db, account, "2026-03-05", "-10.00", None).await;
    common::seed_transaction_label(&db, mine, Some(food), "rule", "applied").await;
    let theirs = common::seed_transaction(&db, other_account, "2026-03-06", "-999.00", None).await;
    common::seed_transaction_label(&db, theirs, Some(food), "rule", "applied").await;

    let out = run(&schema, &user, r#"filter: { startDate: "2026-03-01", endDate: "2026-03-31" }"#).await;
    assert_eq!(amounts(cat(&out, "cmpscope")), [dec_s("10")]);
    assert_eq!(dec(&out["periods"][0]["total"]), dec_s("10"));
}

#[tokio::test]
async fn february_in_a_leap_year_and_a_year_boundary_enumerate_correctly() {
    let db = common::db().await;
    let (user, _) = user_with_account(&db, "cmp-cal").await;
    let schema = create_schema(db.clone(), common::dummy_settings());

    let out = run(&schema, &user, r#"filter: { startDate: "2027-12-01", endDate: "2028-02-29" }"#).await;
    let ends: Vec<_> =
        out["periods"].as_array().unwrap().iter().map(|p| p["end"].as_str().unwrap().to_string()).collect();
    assert_eq!(ends, ["2027-12-31", "2028-01-31", "2028-02-29"]);
    assert!(out["categories"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_missing_date_or_too_many_periods_is_a_validation_error() {
    let db = common::db().await;
    let (user, _) = user_with_account(&db, "cmp-bad").await;
    let schema = create_schema(db.clone(), common::dummy_settings());

    for args in [
        r#"filter: { startDate: "2026-01-01" }"#,
        r#"filter: { startDate: "2026-02-01", endDate: "2026-01-01" }"#,
        r#"filter: { startDate: "2026-01-01", endDate: "2026-12-31" }, granularity: DAY"#,
    ] {
        let q = format!("{{ categoryComparison({args}) {{ currency }} }}");
        let response = schema
            .execute(request_with_auth(Request::new(q), Some(user.clone()), RawSessionToken::default()))
            .await;
        assert!(!response.errors.is_empty(), "{args} should be rejected");
    }
}
