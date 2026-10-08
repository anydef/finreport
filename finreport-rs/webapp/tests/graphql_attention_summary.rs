//! `attentionSummary`: uncategorised (all three shapes) and held-for-review
//! counts + worth, scoped to the caller's accounts, zero when empty.
#![cfg(feature = "integration")]

mod common;

use async_graphql::Request;
use entity::entities::transaction_split;
use sea_orm::{ActiveModelTrait, Set};
use uuid::Uuid;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, RawSessionToken};

async fn summary(schema: &webapp::graphql::AppSchema, user: &AuthenticatedUser) -> serde_json::Value {
    let q = "{ attentionSummary { uncategorized { count totalAmount } needsReview { count totalAmount } } }";
    let response = schema
        .execute(request_with_auth(Request::new(q), Some(user.clone()), RawSessionToken::default()))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()["attentionSummary"].clone()
}

fn amount(v: &serde_json::Value) -> f64 {
    v.as_str().unwrap().parse::<f64>().unwrap()
}

async fn user_with_account(db: &sea_orm::DatabaseConnection, name: &str) -> (AuthenticatedUser, Uuid) {
    let (user_id, username) = common::seed_user(db, name, "pw").await;
    let account = common::seed_account(db, "EUR", name).await;
    common::link(db, user_id, account).await;
    (AuthenticatedUser { user_id, username, account_ids: vec![account] }, account)
}

#[tokio::test]
async fn nothing_to_do_is_all_zero() {
    let db = common::db().await;
    let (user, account) = user_with_account(&db, "attn-zero").await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    let (cat, _) = common::seed_category(&db, "attn_z", "Groceries", "expense").await;
    let t = common::seed_transaction(&db, account, "2024-01-01", "-10.00", None).await;
    common::seed_transaction_label(&db, t, Some(cat), "rule", "applied").await;

    let s = summary(&schema, &user).await;
    assert_eq!(s["uncategorized"]["count"], 0);
    assert_eq!(s["needsReview"]["count"], 0);
    assert_eq!(amount(&s["uncategorized"]["totalAmount"]), 0.0);
    assert_eq!(amount(&s["needsReview"]["totalAmount"]), 0.0);
}

#[tokio::test]
async fn counts_and_sums_uncategorised_in_all_three_shapes_and_held() {
    let db = common::db().await;
    let (user, account) = user_with_account(&db, "attn-count").await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    let (cat, _) = common::seed_category(&db, "attn_c", "Groceries", "expense").await;

    // Uncategorised, shape 1: no label row at all (spending, 10).
    common::seed_transaction(&db, account, "2024-01-01", "-10.00", None).await;
    // Shape 2: a label row with a NULL category (a +20 refund-like credit).
    let null_cat = common::seed_transaction(&db, account, "2024-01-02", "20.00", None).await;
    common::seed_transaction_label(&db, null_cat, None, "llm", "applied").await;
    // Categorised, so excluded.
    let labelled = common::seed_transaction(&db, account, "2024-01-03", "-100.00", None).await;
    common::seed_transaction_label(&db, labelled, Some(cat), "rule", "applied").await;
    // Shape 3: a valid split counts as categorised (label category is NULL).
    let split = common::seed_transaction(&db, account, "2024-01-04", "-50.00", None).await;
    common::seed_transaction_label(&db, split, None, "user", "applied").await;
    for (index, part) in ["-30.00", "-20.00"].iter().enumerate() {
        transaction_split::ActiveModel {
            id: Set(Uuid::new_v4()),
            transaction_id: Set(split),
            part_index: Set(index as i32),
            amount: Set(part.parse().unwrap()),
            category_id: Set(cat),
            invalid: Set(false),
        }
        .insert(&*db)
        .await
        .unwrap();
    }
    // Held for review (categorised, worth 7): counted as held only.
    let held = common::seed_transaction(&db, account, "2024-01-05", "-7.00", None).await;
    common::seed_transaction_label(&db, held, Some(cat), "llm", "needs_review").await;
    // Held and uncategorised (worth 3): counted in both buckets.
    let held_blank = common::seed_transaction(&db, account, "2024-01-06", "-3.00", None).await;
    common::seed_transaction_label(&db, held_blank, None, "llm", "needs_review").await;

    let s = summary(&schema, &user).await;
    // signed net: -10 + 20 - 3 = 7 (the income cancels spending, like a refund)
    assert_eq!(s["uncategorized"]["count"], 3);
    assert_eq!(amount(&s["uncategorized"]["totalAmount"]), 7.0);
    // signed net: -7 - 3 = -10, magnitude 10
    assert_eq!(s["needsReview"]["count"], 2);
    assert_eq!(amount(&s["needsReview"]["totalAmount"]), 10.0);
}

#[tokio::test]
async fn only_the_callers_accounts_are_counted() {
    let db = common::db().await;
    let (user, account) = user_with_account(&db, "attn-mine").await;
    let (_other, other_account) = user_with_account(&db, "attn-theirs").await;
    let schema = create_schema(db.clone(), common::dummy_settings());

    common::seed_transaction(&db, account, "2024-01-01", "-5.00", None).await;
    for _ in 0..4 {
        let t = common::seed_transaction(&db, other_account, "2024-01-01", "-5.00", None).await;
        common::seed_transaction_label(&db, t, None, "llm", "needs_review").await;
    }

    let s = summary(&schema, &user).await;
    assert_eq!(s["uncategorized"]["count"], 1);
    assert_eq!(s["needsReview"]["count"], 0);
}
