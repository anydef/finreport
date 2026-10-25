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

/// One held transaction with no IBAN. `reason`/`proposed` land on its `needs_review` label.
async fn held(
    db: &DatabaseConnection,
    account: Uuid,
    key: Option<&str>,
    name: Option<&str>,
    amount: &str,
    reason: &str,
    proposed: Option<&str>,
) -> Uuid {
    held_iban(db, account, key, None, name, amount, reason, proposed).await
}

/// As `held`, with the counterparty IBAN set (synthetic placeholders only).
#[allow(clippy::too_many_arguments)]
async fn held_iban(
    db: &DatabaseConnection,
    account: Uuid,
    key: Option<&str>,
    iban: Option<&str>,
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
    model.counterparty_iban = Set(iban.map(str::to_string));
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

const GROUPS: &str = "{ heldMerchantGroups { groupCount heldCount groups { counterpartyKey displayName rawName \
    heldCount totalAmount currency reviewReasons proposedCategoryPath proposedCategoryVotes counterpartyIban ibanDisambiguated } } \
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
async fn a_group_shows_the_callers_alias_and_still_exposes_the_bank_name() {
    use webapp::kafka::labeling::{AliasKind, DisplayAliasRecord, CURRENT_SCHEMA_VERSION};
    use webapp::projection::display_alias::project_display_alias;

    let db = common::db().await;
    let a = tenant(&db, "held-alias-a").await;
    let b = tenant(&db, "held-alias-b").await;
    held(&db, a.account, Some("amazon"), Some("Amazon Payments"), "-10.00", "ambiguous", None).await;
    held(&db, a.account, Some("kiosk"), Some("Kiosk"), "-2.00", "ambiguous", None).await;
    held(&db, a.account, None, Some("Mystery"), "-7.00", "ambiguous", None).await;
    held(&db, b.account, Some("amazon"), Some("Amazon Payments"), "-1.00", "ambiguous", None).await;
    let record = DisplayAliasRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        user_id: a.user.user_id,
        kind: AliasKind::Counterparty,
        key: "amazon".to_string(),
        alias: "Shopping".to_string(),
        revision: Utc::now(),
    };
    project_display_alias(db.as_ref(), a.user.user_id, AliasKind::Counterparty, "amazon", Some(record))
        .await
        .unwrap();

    let schema = create_schema(db.clone(), common::dummy_settings());
    let groups = run(&schema, &a.user, GROUPS).await["heldMerchantGroups"]["groups"].clone();
    let by_key = |key: Option<&str>| {
        groups
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["counterpartyKey"].as_str() == key)
            .unwrap_or_else(|| panic!("no group for {key:?}: {groups}"))
            .clone()
    };
    let amazon = by_key(Some("amazon"));
    assert_eq!(amazon["displayName"], "Shopping");
    assert_eq!(amazon["rawName"], "Amazon Payments", "the bank name stays available");
    let kiosk = by_key(Some("kiosk"));
    assert_eq!((&kiosk["displayName"], &kiosk["rawName"]), (&Value::from("Kiosk"), &Value::from("Kiosk")));
    let ungrouped = by_key(None);
    assert_eq!(ungrouped["displayName"], "No merchant key");
    assert_eq!(ungrouped["rawName"], "No merchant key");

    // Another user's alias for the same key does not show up for Bob.
    let bobs = run(&schema, &b.user, GROUPS).await["heldMerchantGroups"]["groups"].clone();
    assert_eq!(bobs[0]["displayName"], "Amazon Payments");
    assert_eq!(bobs[0]["rawName"], "Amazon Payments");
}

const IBAN_A: &str = "DE00 0000 0000 0000 0000 01";
const IBAN_B: &str = "DE00 0000 0000 0000 0000 02";
const IBAN_C: &str = "DE00 0000 0000 0000 0000 03";

/// The filter a client sends for a group: the three cases of the module doc.
/// Mirrors `groupFilter` in `finreport-fe/src/lib/heldGroups.ts`.
fn filter_for(group: &Value) -> String {
    let key = group["counterpartyKey"].as_str().expect("keyed group");
    let mut f = format!(r#"needsReview: true, counterpartyKeys: ["{key}"]"#);
    if group["ibanDisambiguated"] == true {
        match group["counterpartyIban"].as_str() {
            Some(iban) => f.push_str(&format!(r#", counterpartyIbans: ["{iban}"]"#)),
            None => f.push_str(", hasCounterpartyIban: false"),
        }
    }
    f
}

#[tokio::test]
async fn a_group_matches_the_rows_the_bulk_filter_would_assign() {
    let db = common::db().await;
    let a = tenant(&db, "held-groups-filter").await;
    held(&db, a.account, Some("aldi"), Some("Aldi"), "-4.00", "ambiguous", None).await;
    held(&db, a.account, Some("aldi"), Some("Aldi"), "-6.00", "ambiguous", None).await;
    held(&db, a.account, Some("dm"), Some("dm"), "-8.00", "ambiguous", None).await;
    // A lone IBAN under its key: not split, key alone must address it.
    held_iban(&db, a.account, Some("kiosk"), Some(IBAN_C), Some("Kiosk"), "-1.00", "ambiguous", None).await;
    // A split key: same name, two accounts and a no-IBAN row (empty string
    // counts as none). Counts differ so a mixed-up group cannot match by luck.
    for amount in ["-100.00", "-200.00", "-300.00"] {
        held_iban(&db, a.account, Some("owner"), Some(IBAN_A), Some("Max Muster"), amount, "ambiguous", None).await;
    }
    for amount in ["-10.00", "-20.00"] {
        held_iban(&db, a.account, Some("owner"), Some(IBAN_B), Some("Max Muster"), amount, "ambiguous", None).await;
    }
    held_iban(&db, a.account, Some("owner"), Some(""), Some("Max Muster"), "-5.00", "ambiguous", None).await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let groups = run(
        &schema,
        &a.user,
        "{ heldMerchantGroups { groups { counterpartyKey counterpartyIban ibanDisambiguated heldCount } } }",
    )
    .await;
    let groups = groups["heldMerchantGroups"]["groups"].as_array().unwrap().clone();
    assert_eq!(groups.len(), 6, "aldi, dm, kiosk and three owner groups");
    for group in &groups {
        let filter = filter_for(group);
        let q = format!("{{ transactions(filter: {{ {filter} }}) {{ totalCount }} }}");
        let page = run(&schema, &a.user, &q).await;
        assert_eq!(page["transactions"]["totalCount"], group["heldCount"], "group {group}");
    }
    // The filters of the split key are disjoint and cover its 6 rows.
    let owner: Vec<&Value> = groups.iter().filter(|g| g["counterpartyKey"] == "owner").collect();
    assert_eq!(owner.len(), 3);
    assert_eq!(owner.iter().map(|g| g["heldCount"].as_i64().unwrap()).sum::<i64>(), 6);
}

#[tokio::test]
async fn a_key_with_two_ibans_splits_and_a_key_with_one_does_not() {
    let db = common::db().await;
    let a = tenant(&db, "held-groups-split").await;
    // "loan": the user's own name on two accounts, 2 + 1 rows, proposals differ per group.
    held_iban(&db, a.account, Some("loan"), Some(IBAN_A), Some("Max Muster"), "-500.00", "ambiguous", Some("housing.mortgage")).await;
    held_iban(&db, a.account, Some("loan"), Some(IBAN_A), Some("Max Muster"), "-500.00", "ambiguous", Some("housing.mortgage")).await;
    held_iban(&db, a.account, Some("loan"), Some(IBAN_B), Some("Max Muster"), "250.00", "ambiguous", Some("income.other")).await;
    // "kiosk": one IBAN across both rows: a single, unflagged group.
    held_iban(&db, a.account, Some("kiosk"), Some(IBAN_C), Some("Kiosk"), "-2.00", "ambiguous", None).await;
    held_iban(&db, a.account, Some("kiosk"), Some(IBAN_C), Some("Kiosk"), "-3.00", "ambiguous", None).await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let data = run(&schema, &a.user, GROUPS).await;
    let result = &data["heldMerchantGroups"];
    let groups = result["groups"].as_array().unwrap();

    assert_eq!(result["groupCount"], 3, "the pairs, not the keys");
    assert_eq!(result["heldCount"], 5);
    assert_eq!(data["reviewQueue"]["totalCount"], 5);
    assert_eq!(groups.iter().map(|g| g["heldCount"].as_i64().unwrap()).sum::<i64>(), 5);
    assert_eq!(groups.len(), 3);

    let find = |key: &str, iban: &str| {
        groups
            .iter()
            .find(|g| g["counterpartyKey"] == key && g["counterpartyIban"] == iban)
            .unwrap_or_else(|| panic!("no group {key}/{iban}: {groups:?}"))
    };
    let a_group = find("loan", IBAN_A);
    let b_group = find("loan", IBAN_B);
    assert_eq!((a_group["heldCount"].as_i64(), b_group["heldCount"].as_i64()), (Some(2), Some(1)));
    assert_eq!(a_group["ibanDisambiguated"], true);
    assert_eq!(b_group["ibanDisambiguated"], true);
    // Votes follow the pair: A's two agree, B's one is its own.
    assert_eq!(a_group["proposedCategoryPath"], "housing.mortgage");
    assert_eq!(a_group["proposedCategoryVotes"], 2);
    assert_eq!(b_group["proposedCategoryPath"], "income.other");
    assert_eq!(b_group["proposedCategoryVotes"], 1);
    assert_eq!(b_group["totalAmount"].as_str().unwrap().parse::<f64>().unwrap(), 250.0);

    let kiosk = find("kiosk", IBAN_C);
    assert_eq!(kiosk["heldCount"], 2);
    assert_eq!(kiosk["ibanDisambiguated"], false, "one IBAN under the key is not a split");
}

#[tokio::test]
async fn a_no_iban_group_with_null_votes_counts_and_is_flagged_beside_its_iban_sibling() {
    let db = common::db().await;
    let a = tenant(&db, "held-groups-noiban").await;
    held_iban(&db, a.account, Some("loan"), Some(IBAN_A), Some("Max Muster"), "-1.00", "ambiguous", Some("x.y")).await;
    held_iban(&db, a.account, Some("loan"), None, Some("Max Muster"), "-2.00", "ambiguous", Some("x.y")).await;
    held_iban(&db, a.account, Some("loan"), None, Some("Max Muster"), "-3.00", "ambiguous", Some("x.y")).await;
    // Two keyless rows with the same IBAN and one with none: two keyless groups.
    held_iban(&db, a.account, None, Some(IBAN_B), Some("Mystery"), "-4.00", "ambiguous", None).await;
    held_iban(&db, a.account, None, Some(IBAN_B), Some("Mystery"), "-5.00", "ambiguous", None).await;
    held_iban(&db, a.account, None, None, Some("Mystery"), "-6.00", "ambiguous", None).await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let data = run(&schema, &a.user, GROUPS).await;
    let result = &data["heldMerchantGroups"];
    let groups = result["groups"].as_array().unwrap();
    assert_eq!(result["groupCount"], 4);
    assert_eq!(result["heldCount"], 6);
    assert_eq!(groups.len(), 4);
    assert_eq!(groups.iter().map(|g| g["heldCount"].as_i64().unwrap()).sum::<i64>(), 6);

    let no_iban = groups
        .iter()
        .find(|g| g["counterpartyKey"] == "loan" && g["counterpartyIban"].is_null())
        .expect("the loan's no-IBAN group");
    assert_eq!(no_iban["heldCount"], 2);
    assert_eq!(no_iban["ibanDisambiguated"], true);
    assert_eq!(
        no_iban["proposedCategoryVotes"], 2,
        "votes must still join on a NULL IBAN (IS NOT DISTINCT FROM), not drop to zero"
    );
    let keyless_iban = groups
        .iter()
        .find(|g| g["counterpartyKey"].is_null() && g["counterpartyIban"] == IBAN_B)
        .expect("keyless group with an IBAN");
    assert_eq!(keyless_iban["heldCount"], 2);
    assert_eq!(keyless_iban["displayName"], "No merchant key");
}

#[tokio::test]
async fn a_sibling_on_another_page_still_marks_the_group_disambiguated() {
    let db = common::db().await;
    let a = tenant(&db, "held-groups-sibling-page").await;
    // Sorted by heldCount, the two groups of "loan" land on different pages of size 1.
    held_iban(&db, a.account, Some("loan"), Some(IBAN_A), Some("Max Muster"), "-1.00", "ambiguous", None).await;
    held_iban(&db, a.account, Some("loan"), Some(IBAN_A), Some("Max Muster"), "-2.00", "ambiguous", None).await;
    held_iban(&db, a.account, Some("loan"), Some(IBAN_B), Some("Max Muster"), "-3.00", "ambiguous", None).await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    for (offset, count, iban) in [(0, 2, IBAN_A), (1, 1, IBAN_B)] {
        let q = format!(
            "{{ heldMerchantGroups(page: {{ limit: 1, offset: {offset} }}) {{ groupCount groups {{ counterpartyIban ibanDisambiguated heldCount }} }} }}"
        );
        let data = run(&schema, &a.user, &q).await;
        let groups = data["heldMerchantGroups"]["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["counterpartyIban"], iban);
        assert_eq!(groups[0]["heldCount"], count);
        assert_eq!(groups[0]["ibanDisambiguated"], true, "page offset {offset}");
        assert_eq!(data["heldMerchantGroups"]["groupCount"], 2);
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
