//! `attentionSummary`: uncategorised (all three shapes) and held-for-review
//! counts + worth, scoped to the caller's accounts, zero when empty.
#![cfg(feature = "integration")]

mod common;

use async_graphql::Request;
use entity::entities::transaction_split;
use sea_orm::{ActiveModelTrait, ColumnTrait, QueryFilter, Set};
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

async fn rules_learned(schema: &webapp::graphql::AppSchema, user: &AuthenticatedUser) -> i64 {
    let q = "{ attentionSummary { rulesLearned } }";
    let response = schema
        .execute(request_with_auth(Request::new(q), Some(user.clone()), RawSessionToken::default()))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()["attentionSummary"]["rulesLearned"].as_i64().expect("rulesLearned is a non-null Int")
}

async fn seed_rule(db: &sea_orm::DatabaseConnection, category_id: Uuid, origin: &str, state: &str) -> Uuid {
    let id = Uuid::new_v4();
    entity::entities::rule::ActiveModel {
        id: Set(id),
        name: Set(format!("attn-rule-{id}")),
        category_id: Set(category_id),
        conditions: Set(serde_json::json!({})),
        priority: Set(0),
        state: Set(state.to_string()),
        origin: Set(origin.to_string()),
        auto_approved: Set(false),
        user_touched: Set(false),
        confidence: Set(None),
        evidence: Set(None),
        created_at: Set(chrono::Utc::now().into()),
        revision: Set(chrono::Utc::now().into()),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

/// Only `learned` + `active` counts. The shared test database may hold rules
/// from other suites, so every assertion is a delta from a baseline read.
#[tokio::test]
async fn rules_learned_counts_only_active_learned_rules() {
    use entity::entities::rule;
    use sea_orm::EntityTrait;

    let db = common::db().await;
    let (user, _) = user_with_account(&db, "attn-rules").await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    let (cat, _) = common::seed_category(&db, "attn_r", "Groceries", "expense").await;

    // Non-null Int even with nothing of ours seeded (0 on an empty table).
    let baseline = rules_learned(&schema, &user).await;
    assert!(baseline >= 0);

    let mut ids = Vec::new();
    for state in ["in_review", "revoked", "rejected"] {
        ids.push(seed_rule(&db, cat, "learned", state).await);
    }
    ids.push(seed_rule(&db, cat, "user", "active").await);
    assert_eq!(rules_learned(&schema, &user).await, baseline, "non-active learned and user rules are not counted");

    ids.push(seed_rule(&db, cat, "learned", "active").await);
    assert_eq!(rules_learned(&schema, &user).await, baseline + 1);

    ids.push(seed_rule(&db, cat, "learned", "active").await);
    assert_eq!(rules_learned(&schema, &user).await, baseline + 2);

    rule::Entity::delete_many().filter(rule::Column::Id.is_in(ids)).exec(&*db).await.unwrap();
    assert_eq!(rules_learned(&schema, &user).await, baseline);
}

/// A user with no accounts still gets a number, not null or an error.
#[tokio::test]
async fn rules_learned_is_an_int_for_a_user_with_no_accounts() {
    let db = common::db().await;
    let (user_id, username) = common::seed_user(&db, "attn-rules-noacct", "pw").await;
    let user = AuthenticatedUser { user_id, username, account_ids: vec![] };
    let schema = create_schema(db.clone(), common::dummy_settings());
    assert!(rules_learned(&schema, &user).await >= 0);
}
