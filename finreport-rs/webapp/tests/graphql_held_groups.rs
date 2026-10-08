//! `heldMerchantGroups`: the review queue grouped by merchant. Postgres only
//! (labels are seeded directly), no Kafka needed. Needs the throwaway test
//! container described in `tests/common/mod.rs`.
#![cfg(feature = "integration")]

mod common;

use async_graphql::Request;
use chrono::Utc;
use entity::entities::{transaction, transaction_label};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use serde_json::Value;
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
    Tenant { user: AuthenticatedUser { user_id, username, account_ids: vec![account] }, account }
}

/// One held transaction. `reason`/`proposed` land on its `needs_review` label.
async fn held(
    db: &DatabaseConnection,
    account: Uuid,
    key: Option<&str>,
    name: Option<&str>,
    amount: &str,
    reason: &str,
    proposed: Option<&str>,
) -> Uuid {
    let id = common::seed_transaction(db, account, "2024-07-01", amount, name).await;
    let mut model: transaction::ActiveModel =
        transaction::Entity::find_by_id(id).one(db).await.unwrap().unwrap().into();
    model.counterparty_key = Set(key.map(str::to_string));
    model.counterparty_name = Set(name.map(str::to_string));
    model.update(db).await.unwrap();
    common::seed_transaction_label(db, id, None, "llm", "needs_review").await;
    let mut label: transaction_label::ActiveModel =
        transaction_label::Entity::find_by_id(id).one(db).await.unwrap().unwrap().into();
    label.review_reason = Set(Some(reason.to_string()));
    label.proposed_category_path = Set(proposed.map(str::to_string));
    label.labeled_at = Set(Utc::now().into());
    label.update(db).await.unwrap();
    id
}

async fn run(schema: &webapp::graphql::AppSchema, user: &AuthenticatedUser, query: &str) -> Value {
    let response = schema
        .execute(request_with_auth(
            Request::new(query),
            Some(user.clone()),
            RawSessionToken::default(),
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    response.data.into_json().unwrap()
}

const GROUPS: &str = "{ heldMerchantGroups { groupCount heldCount groups { counterpartyKey displayName \
    heldCount totalAmount currency reviewReasons proposedCategoryPath proposedCategoryVotes } } \
    reviewQueue { totalCount } }";

#[tokio::test]
async fn groups_count_sum_scope_and_bucket_the_keyless() {
    let db = common::db().await;
    let a = tenant(&db, "held-groups-a").await;
    let b = tenant(&db, "held-groups-b").await;

    // Merchant "rewe": 3 held, names disagree ("REWE Markt" twice wins over
    // "Rewe"), proposals disagree ("food.groceries" x2, "food.dining" x1).
    held(&db, a.account, Some("rewe"), Some("REWE Markt"), "-10.00", "ambiguous", Some("food.groceries")).await;
    held(&db, a.account, Some("rewe"), Some("REWE Markt"), "-20.50", "ambiguous", Some("food.groceries")).await;
    held(&db, a.account, Some("rewe"), Some("Rewe"), "-5.00", "new_category", Some("food.dining")).await;
    // A group of one with no proposal.
    held(&db, a.account, Some("kiosk"), Some("Kiosk"), "-2.00", "ambiguous", None).await;
    // Two keyless transactions (NULL and empty) share one bucket.
    held(&db, a.account, None, Some("Mystery"), "-7.00", "ambiguous", None).await;
    held(&db, a.account, Some(""), None, "-3.00", "ambiguous", None).await;
    // A resolved transaction is not held and must not be counted.
    let resolved = common::seed_transaction(&db, a.account, "2024-07-01", "-99.00", Some("REWE Markt")).await;
    common::seed_transaction_label(&db, resolved, None, "llm", "resolved").await;
    // Another tenant's held rows, same merchant key: must not leak in.
    held(&db, b.account, Some("rewe"), Some("REWE Markt"), "-1000.00", "ambiguous", None).await;
    held(&db, b.account, Some("other"), Some("Other"), "-1.00", "ambiguous", None).await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let data = run(&schema, &a.user, GROUPS).await;
    let result = &data["heldMerchantGroups"];
    let groups = result["groups"].as_array().unwrap();

    assert_eq!(result["heldCount"], 6);
    assert_eq!(result["groupCount"], 3);
    assert_eq!(data["reviewQueue"]["totalCount"], 6, "groups must agree with the queue total");
    let summed: i64 = groups.iter().map(|g| g["heldCount"].as_i64().unwrap()).sum();
    assert_eq!(summed, 6, "group counts must sum to the held total");

    // Largest first.
    let counts: Vec<i64> = groups.iter().map(|g| g["heldCount"].as_i64().unwrap()).collect();
    assert_eq!(counts, vec![3, 2, 1]);

    let rewe = &groups[0];
    assert_eq!(rewe["counterpartyKey"], "rewe");
    assert_eq!(rewe["displayName"], "REWE Markt", "most common name");
    assert_eq!(rewe["totalAmount"].as_str().unwrap().parse::<f64>().unwrap(), -35.5);
    assert_eq!(rewe["currency"], "EUR");
    assert_eq!(rewe["proposedCategoryPath"], "food.groceries");
    assert_eq!(rewe["proposedCategoryVotes"], 2, "disagreement is visible: 2 of 3");
    assert_eq!(rewe["reviewReasons"], serde_json::json!(["AMBIGUOUS", "NEW_CATEGORY"]));

    let ungrouped = &groups[1];
    assert!(ungrouped["counterpartyKey"].is_null());
    assert_eq!(ungrouped["displayName"], "No merchant key");
    assert_eq!(ungrouped["totalAmount"].as_str().unwrap().parse::<f64>().unwrap(), -10.0);

    let kiosk = &groups[2];
    assert_eq!(kiosk["counterpartyKey"], "kiosk");
    assert!(kiosk["proposedCategoryPath"].is_null());
    assert_eq!(kiosk["proposedCategoryVotes"], 0);

    // The other tenant sees only their own two groups.
    let data_b = run(&schema, &b.user, GROUPS).await;
    assert_eq!(data_b["heldMerchantGroups"]["heldCount"], 2);
    assert_eq!(data_b["heldMerchantGroups"]["groupCount"], 2);
}

#[tokio::test]
async fn a_group_matches_the_rows_the_bulk_filter_would_assign() {
    let db = common::db().await;
    let a = tenant(&db, "held-groups-filter").await;
    held(&db, a.account, Some("aldi"), Some("Aldi"), "-4.00", "ambiguous", None).await;
    held(&db, a.account, Some("aldi"), Some("Aldi"), "-6.00", "ambiguous", None).await;
    held(&db, a.account, Some("dm"), Some("dm"), "-8.00", "ambiguous", None).await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let groups = run(&schema, &a.user, "{ heldMerchantGroups { groups { counterpartyKey heldCount } } }").await;
    for group in groups["heldMerchantGroups"]["groups"].as_array().unwrap() {
        let key = group["counterpartyKey"].as_str().unwrap();
        let q = format!(
            r#"{{ transactions(filter: {{ needsReview: true, counterpartyKeys: ["{key}"] }}) {{ totalCount }} }}"#
        );
        let page = run(&schema, &a.user, &q).await;
        assert_eq!(page["transactions"]["totalCount"], group["heldCount"], "group {key}");
    }
}

#[tokio::test]
async fn paging_walks_the_groups_and_totals_survive_a_page_past_the_end() {
    let db = common::db().await;
    let a = tenant(&db, "held-groups-paging").await;
    for key in ["k1", "k2", "k3"] {
        held(&db, a.account, Some(key), Some(key), "-1.00", "ambiguous", None).await;
    }
    let schema = create_schema(db.clone(), common::dummy_settings());
    let page = run(
        &schema,
        &a.user,
        "{ heldMerchantGroups(page: { limit: 2, offset: 0 }) { groups { counterpartyKey } groupCount heldCount } }",
    )
    .await;
    assert_eq!(page["heldMerchantGroups"]["groups"].as_array().unwrap().len(), 2);
    assert_eq!(page["heldMerchantGroups"]["groupCount"], 3);

    let past = run(
        &schema,
        &a.user,
        "{ heldMerchantGroups(page: { limit: 2, offset: 50 }) { groups { counterpartyKey } groupCount heldCount } }",
    )
    .await;
    assert!(past["heldMerchantGroups"]["groups"].as_array().unwrap().is_empty());
    assert_eq!(past["heldMerchantGroups"]["heldCount"], 3);
    assert_eq!(past["heldMerchantGroups"]["groupCount"], 3);
}

#[tokio::test]
async fn a_caller_with_no_accounts_gets_an_empty_result() {
    let db = common::db().await;
    let (user_id, username) = common::seed_user(&db, "held-groups-none", "pw").await;
    let user = AuthenticatedUser { user_id, username, account_ids: vec![] };
    let schema = create_schema(db.clone(), common::dummy_settings());
    let data = run(&schema, &user, GROUPS).await;
    assert_eq!(data["heldMerchantGroups"]["heldCount"], 0);
    assert_eq!(data["heldMerchantGroups"]["groups"].as_array().unwrap().len(), 0);
}
