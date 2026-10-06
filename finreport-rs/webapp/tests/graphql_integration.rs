//! GraphQL-level integration tests (§8/§9), gated behind the `integration`
//! feature and requiring the throwaway `finreport-wp4-pg` Postgres
//! container (`127.0.0.1:55435`, see `tests/common/mod.rs`). Executes
//! against the real schema built by `webapp::graphql::create_schema`,
//! seeding fixtures directly via entities (the WP3 projector isn't on this
//! branch).
#![cfg(feature = "integration")]

mod common;

use async_graphql::Request;
use rust_decimal::Decimal;
use std::str::FromStr;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, RawSessionToken};

fn authed(request: Request, user: Option<AuthenticatedUser>) -> Request {
    request_with_auth(request, user, RawSessionToken::default())
}

#[tokio::test]
async fn unauthenticated_access_is_denied() {
    let db = common::db().await;
    let schema = create_schema(db, common::dummy_settings());

    let request = authed(Request::new("{ accounts { id } }"), None);
    let response = schema.execute(request).await;

    assert!(!response.errors.is_empty(), "expected an error, got {response:?}");
    assert_eq!(
        response.errors[0].extensions.as_ref().and_then(|e| e.get("code")),
        Some(&async_graphql::Value::String("UNAUTHENTICATED".to_string()))
    );
}

#[tokio::test]
async fn cross_user_account_access_is_denied() {
    let db = common::db().await;

    let (user_a, _) = common::seed_user(&db, "alice-scope-test", "pw").await;
    let account_a = common::seed_account(&db, "EUR", "Alice's account").await;
    common::link(&db, user_a, account_a).await;

    let (_user_b, _) = common::seed_user(&db, "bob-scope-test", "pw").await;
    let account_b = common::seed_account(&db, "EUR", "Bob's account").await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user_a,
        username: "alice-scope-test".to_string(),
        account_ids: vec![account_a],
    };

    let query = format!(
        r#"{{ transactions(filter: {{ accountIds: ["{account_b}"] }}) {{ totalCount }} }}"#
    );
    let request = authed(Request::new(query), Some(caller));
    let response = schema.execute(request).await;

    assert!(!response.errors.is_empty(), "expected a scoping error, got {response:?}");
    assert!(
        response.errors[0].message.contains("not accessible"),
        "unexpected message: {}",
        response.errors[0].message
    );
}

#[tokio::test]
async fn cashflow_summary_matches_a_hand_computed_fixture_sum() {
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "carol-summary-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Carol's account").await;
    common::link(&db, user, account).await;

    // Hand-computed fixture: three days inside the range, one outside it.
    common::seed_transaction(&db, account, "2024-01-01", "100.00", Some("Employer")).await;
    common::seed_transaction(&db, account, "2024-01-01", "-20.00", Some("Shop")).await;
    common::seed_transaction(&db, account, "2024-01-03", "-5.50", Some("Shop")).await;
    common::seed_transaction(&db, account, "2024-02-01", "999.00", Some("Outside range")).await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "carol-summary-test".to_string(),
        account_ids: vec![account],
    };

    let query = r#"{
        cashflowSummary(
            filter: { startDate: "2024-01-01", endDate: "2024-01-03" }
            granularity: DAY
        ) {
            total { income spending net transactionCount }
            buckets { start income spending net transactionCount }
        }
    }"#;
    let response = schema.execute(authed(Request::new(query), Some(caller))).await;
    assert!(response.errors.is_empty(), "unexpected errors: {:?}", response.errors);

    let data = response.data.into_json().unwrap();
    let total = &data["cashflowSummary"]["total"];
    // income = 100.00; spending = 20.00 + 5.50 = 25.50; net = 74.50.
    // The `Decimal` scalar round-trips at NUMERIC(20,4) precision (§5).
    assert_eq!(total["income"], "100.0000");
    assert_eq!(total["spending"], "25.5000");
    assert_eq!(total["net"], "74.5000");
    assert_eq!(total["transactionCount"], 3);

    let buckets = data["cashflowSummary"]["buckets"].as_array().unwrap();
    // Dense fill: Jan 1, 2, 3 — three buckets, including the empty Jan 2.
    assert_eq!(buckets.len(), 3);
    assert_eq!(buckets[1]["transactionCount"], 0, "Jan 2 has no transactions");
}

#[tokio::test]
async fn cashflow_graph_conserves_flow_and_reconciles_with_summary() {
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "dave-graph-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Dave's account").await;
    common::link(&db, user, account).await;

    common::seed_transaction(&db, account, "2024-03-01", "500.00", Some("Employer")).await;
    common::seed_transaction(&db, account, "2024-03-02", "-120.00", Some("Landlord")).await;
    common::seed_transaction(&db, account, "2024-03-03", "-30.00", Some("Landlord")).await;
    common::seed_transaction(&db, account, "2024-03-04", "-10.00", None).await; // Unknown

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "dave-graph-test".to_string(),
        account_ids: vec![account],
    };

    let filter = r#"{ startDate: "2024-03-01", endDate: "2024-03-04" }"#;

    let graph_query = format!(
        r#"{{ cashflowGraph(filter: {filter}) {{
            nodes {{ id kind value }}
            links {{ sourceId targetId value }}
        }} }}"#
    );
    let graph_response = schema
        .execute(authed(Request::new(graph_query), Some(caller.clone())))
        .await;
    assert!(graph_response.errors.is_empty(), "{:?}", graph_response.errors);
    let graph = graph_response.data.into_json().unwrap();
    let nodes = graph["cashflowGraph"]["nodes"].as_array().unwrap();
    let links = graph["cashflowGraph"]["links"].as_array().unwrap();

    // Flow conservation: every node's total in-value equals its total
    // out-value equals the `value` the server reported for it.
    for node in nodes {
        let id = node["id"].as_str().unwrap();
        let reported: Decimal = Decimal::from_str(node["value"].as_str().unwrap()).unwrap();
        let inflow: Decimal = links
            .iter()
            .filter(|l| l["targetId"] == *id)
            .map(|l| Decimal::from_str(l["value"].as_str().unwrap()).unwrap())
            .sum();
        let outflow: Decimal = links
            .iter()
            .filter(|l| l["sourceId"] == *id)
            .map(|l| Decimal::from_str(l["value"].as_str().unwrap()).unwrap())
            .sum();
        let flow = if inflow > Decimal::ZERO { inflow } else { outflow };
        assert_eq!(
            flow, reported,
            "node {id} does not conserve flow: in={inflow} out={outflow} reported={reported}"
        );
    }

    let summary_query = format!(
        r#"{{ cashflowSummary(filter: {filter}, granularity: DAY) {{
            total {{ income spending }}
        }} }}"#
    );
    let summary_response = schema
        .execute(authed(Request::new(summary_query), Some(caller)))
        .await;
    assert!(summary_response.errors.is_empty(), "{:?}", summary_response.errors);
    let summary = summary_response.data.into_json().unwrap();
    let summary_income =
        Decimal::from_str(summary["cashflowSummary"]["total"]["income"].as_str().unwrap()).unwrap();
    let summary_spending =
        Decimal::from_str(summary["cashflowSummary"]["total"]["spending"].as_str().unwrap()).unwrap();

    // Reconciliation: income flowing in from INCOME_SOURCE/OTHER (not a
    // DEFICIT node filling a shortfall) must equal cashflowSummary's
    // income, and spending flowing out to SPENDING/OTHER (not money kept in
    // a NET node) must equal its spending.
    let node_kind = |id: &serde_json::Value| -> String {
        nodes
            .iter()
            .find(|n| &n["id"] == id)
            .map(|n| n["kind"].as_str().unwrap().to_string())
            .unwrap()
    };
    let total_income_in: Decimal = links
        .iter()
        .filter(|l| {
            node_kind(&l["targetId"]) == "ACCOUNT" && node_kind(&l["sourceId"]) != "DEFICIT"
        })
        .map(|l| Decimal::from_str(l["value"].as_str().unwrap()).unwrap())
        .sum();
    let total_spending_out: Decimal = links
        .iter()
        .filter(|l| {
            node_kind(&l["sourceId"]) == "ACCOUNT" && node_kind(&l["targetId"]) != "NET"
        })
        .map(|l| Decimal::from_str(l["value"].as_str().unwrap()).unwrap())
        .sum();

    assert_eq!(total_income_in, summary_income);
    assert_eq!(total_spending_out, summary_spending);
}
