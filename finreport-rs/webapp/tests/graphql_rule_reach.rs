//! `Rule.matchingTransactionCount` is a rule's reach over the *caller's own*
//! accounts: rules are global (every user sees every rule), so the count is
//! the one field on `Rule` that must not leak another tenant's data.
//! Postgres only (rules are seeded directly), no Kafka needed.
#![cfg(feature = "integration")]

mod common;

use async_graphql::Request;
use chrono::Utc;
use entity::entities::{rule, transaction};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, RawSessionToken};

struct Tenant {
    user: AuthenticatedUser,
    account: Uuid,
}

async fn tenant(db: &DatabaseConnection, name: &str) -> Tenant {
    let (user_id, username) = common::seed_user(db, name, "pw").await;
    let account = common::seed_account(db, "EUR", name).await;
    common::link(db, user_id, account).await;
    Tenant {
        user: AuthenticatedUser { user_id, username, account_ids: vec![account] },
        account,
    }
}

async fn tx_with_key(db: &DatabaseConnection, account: Uuid, key: &str, amount: &str) {
    let id = common::seed_transaction(db, account, "2024-07-01", amount, Some("Shop")).await;
    let mut model: transaction::ActiveModel =
        transaction::Entity::find_by_id(id).one(db).await.unwrap().unwrap().into();
    model.counterparty_key = Set(Some(key.to_string()));
    model.update(db).await.unwrap();
}

async fn seed_rule(
    db: &DatabaseConnection,
    category: Uuid,
    state: &str,
    conditions: serde_json::Value,
) -> Uuid {
    let id = Uuid::new_v4();
    rule::ActiveModel {
        id: Set(id),
        name: Set("reach test".to_string()),
        category_id: Set(category),
        conditions: Set(conditions),
        priority: Set(0),
        state: Set(state.to_string()),
        origin: Set("USER".to_string()),
        auto_approved: Set(false),
        user_touched: Set(true),
        confidence: Set(None),
        evidence: Set(None),
        created_at: Set(Utc::now().into()),
        revision: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert rule");
    id
}

async fn counts(
    schema: &webapp::graphql::AppSchema,
    user: &AuthenticatedUser,
) -> std::collections::HashMap<Uuid, i64> {
    let response = schema
        .execute(request_with_auth(
            Request::new("{ rules { id matchingTransactionCount } }"),
            Some(user.clone()),
            RawSessionToken::default(),
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let json = response.data.into_json().unwrap();
    json["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["id"].as_str().unwrap().parse().unwrap(),
                r["matchingTransactionCount"].as_i64().unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_count_covers_only_the_callers_accounts() {
    let db = common::db().await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    let alice = tenant(&db, "reach-alice").await;
    let bob = tenant(&db, "reach-bob").await;
    let (category, _) = common::seed_category(&db, "reach", "Reach", "EXPENSE").await;
    // Unique per run: the Postgres instance is shared across tests.
    let key = format!("reach-{}", Uuid::new_v4().simple());

    for _ in 0..3 {
        tx_with_key(&db, alice.account, &key, "-10").await;
    }
    tx_with_key(&db, bob.account, &key, "-10").await;
    tx_with_key(&db, bob.account, "someone-else", "-10").await;

    let by_key =
        seed_rule(&db, category, "ACTIVE", serde_json::json!({"counterparty_key": key})).await;
    // Reach ignores state: a revoked rule still has a reach.
    let revoked =
        seed_rule(&db, category, "REVOKED", serde_json::json!({"counterparty_key": key})).await;
    // Another tenant's account in `account_ids` can never widen the caller's reach.
    let bobs_account_only = seed_rule(
        &db,
        category,
        "ACTIVE",
        serde_json::json!({"counterparty_key": key, "account_ids": [bob.account]}),
    )
    .await;
    let income = seed_rule(
        &db,
        category,
        "ACTIVE",
        serde_json::json!({"counterparty_key": key, "direction": "INCOME"}),
    )
    .await;

    let alice_counts = counts(&schema, &alice.user).await;
    assert_eq!(alice_counts[&by_key], 3, "alice must not count bob's transaction");
    assert_eq!(alice_counts[&revoked], 3);
    assert_eq!(alice_counts[&bobs_account_only], 0, "bob's account is outside alice's scope");
    assert_eq!(alice_counts[&income], 0, "all of alice's matching transactions are spending");

    let bob_counts = counts(&schema, &bob.user).await;
    assert_eq!(bob_counts[&by_key], 1);
    assert_eq!(bob_counts[&bobs_account_only], 1);
}

#[tokio::test]
async fn a_caller_with_no_accounts_sees_zero_reach() {
    let db = common::db().await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    let (user_id, username) = common::seed_user(&db, "reach-nobody", "pw").await;
    let nobody = AuthenticatedUser { user_id, username, account_ids: vec![] };
    let (category, _) = common::seed_category(&db, "reach", "Reach", "EXPENSE").await;
    let everything = seed_rule(&db, category, "ACTIVE", serde_json::json!({})).await;

    assert_eq!(counts(&schema, &nobody).await[&everything], 0);
}
