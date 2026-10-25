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
use std::sync::Arc;
use std::time::Duration;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, RawSessionToken};
use webapp::kafka::labeling::{TOPIC_CATEGORY, TOPIC_LABEL_REQUEST, TOPIC_RULE, TOPIC_USER_LABEL};

fn authed(request: Request, user: Option<AuthenticatedUser>) -> Request {
    request_with_auth(request, user, RawSessionToken::default())
}

/// Starts a throwaway Kafka broker (own container, not the shared
/// `support::kafka::TestKafka` — that helper only creates the four
/// iteration-1 topics and isn't WP4's to extend) with the four iteration-2
/// labeling topics the mutations under test publish to. Returns the
/// `host:port` string for `Settings.kafka_brokers` / `create_schema`.
async fn start_kafka_with_labeling_topics(
) -> (testcontainers::ContainerAsync<testcontainers_modules::kafka::apache::Kafka>, String) {
    use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
    use rdkafka::client::DefaultClientContext;
    use rdkafka::ClientConfig;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::kafka::apache::{Kafka, KAFKA_PORT};

    let container = Kafka::default().start().await.expect("start Kafka testcontainer");
    let host = container.get_host().await.expect("Kafka testcontainer host");
    let port = container
        .get_host_port_ipv4(KAFKA_PORT)
        .await
        .expect("Kafka testcontainer mapped port");
    let bootstrap_servers = format!("{host}:{port}");

    let admin: AdminClient<DefaultClientContext> = ClientConfig::new()
        .set("bootstrap.servers", &bootstrap_servers)
        .create()
        .expect("create Kafka admin client");
    let topics = [TOPIC_USER_LABEL, TOPIC_RULE, TOPIC_CATEGORY, TOPIC_LABEL_REQUEST]
        .map(|name| NewTopic::new(name, 1, TopicReplication::Fixed(1)));
    admin
        .create_topics(&topics, &AdminOptions::new().request_timeout(Some(Duration::from_secs(10))))
        .await
        .expect("create labeling topics on Kafka testcontainer");

    (container, bootstrap_servers)
}

fn settings_with_kafka(bootstrap_servers: &str) -> Arc<utils::settings::Settings> {
    let mut settings = (*common::dummy_settings()).clone();
    settings.kafka_brokers = Some(bootstrap_servers.to_string());
    Arc::new(settings)
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

#[tokio::test]
async fn set_transaction_category_without_kafka_fails_with_kafka_unavailable() {
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "erin-nokafka-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Erin's account").await;
    common::link(&db, user, account).await;
    let tx = common::seed_transaction(&db, account, "2024-04-01", "-10.00", Some("Shop")).await;
    let (_, slug) = common::seed_category(&db, "expense.groceries", "Groceries", "EXPENSE").await;

    // `common::dummy_settings()` leaves `kafka_brokers` unset (§8), so the
    // schema is built with no publisher at all — the mutation must fail
    // fast with `KAFKA_UNAVAILABLE` rather than panicking.
    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "erin-nokafka-test".to_string(),
        account_ids: vec![account],
    };
    let query = format!(
        r#"mutation {{ setTransactionCategory(transactionId: "{tx}", categorySlug: "{slug}") {{ id }} }}"#
    );
    let response = schema.execute(authed(Request::new(query), Some(caller))).await;

    assert!(!response.errors.is_empty(), "expected an error, got {response:?}");
    assert_eq!(
        response.errors[0].extensions.as_ref().and_then(|e| e.get("code")),
        Some(&async_graphql::Value::String("KAFKA_UNAVAILABLE".to_string()))
    );
}

#[tokio::test]
async fn split_transaction_rejects_a_sum_mismatch_and_splits_round_trip_via_the_split_field() {
    let (_kafka, bootstrap) = start_kafka_with_labeling_topics().await;
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "frank-split-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Frank's account").await;
    common::link(&db, user, account).await;
    let tx = common::seed_transaction(&db, account, "2024-04-02", "-100.00", Some("Supermarket")).await;
    let (_, groceries_slug) = common::seed_category(&db, "expense.groceries2", "Groceries", "EXPENSE").await;
    let (_, household_slug) = common::seed_category(&db, "expense.household2", "Household", "EXPENSE").await;

    let schema = create_schema(db.clone(), settings_with_kafka(&bootstrap));
    let caller = AuthenticatedUser {
        user_id: user,
        username: "frank-split-test".to_string(),
        account_ids: vec![account],
    };

    // Sum mismatch: parts sum to -90.00, not the transaction's -100.00.
    let bad_query = format!(
        r#"mutation {{ splitTransaction(transactionId: "{tx}", parts: [
            {{ amount: "-60.00", categorySlug: "{groceries_slug}" }},
            {{ amount: "-30.00", categorySlug: "{household_slug}" }}
        ]) {{ id }} }}"#
    );
    let bad_response = schema
        .execute(authed(Request::new(bad_query), Some(caller.clone())))
        .await;
    assert!(!bad_response.errors.is_empty(), "expected a sum-mismatch error");
    assert_eq!(
        bad_response.errors[0].extensions.as_ref().and_then(|e| e.get("code")),
        Some(&async_graphql::Value::String("SPLIT_SUM_MISMATCH".to_string()))
    );

    // Exact sum: parts sum to exactly -100.00.
    let good_query = format!(
        r#"mutation {{ splitTransaction(transactionId: "{tx}", parts: [
            {{ amount: "-70.00", categorySlug: "{groceries_slug}" }},
            {{ amount: "-30.00", categorySlug: "{household_slug}" }}
        ]) {{ splits {{ index amount category {{ slug }} }} }} }}"#
    );
    let good_response = schema
        .execute(authed(Request::new(good_query), Some(caller.clone())))
        .await;
    assert!(good_response.errors.is_empty(), "{:?}", good_response.errors);
    let data = good_response.data.into_json().unwrap();
    let splits = data["splitTransaction"]["splits"].as_array().unwrap();
    assert_eq!(splits.len(), 2);
    let sum: Decimal = splits
        .iter()
        .map(|s| Decimal::from_str(s["amount"].as_str().unwrap()).unwrap())
        .sum();
    assert_eq!(sum, Decimal::from_str("-100.00").unwrap());

    // unsplitTransaction clears the parts back out.
    let unsplit_query =
        format!(r#"mutation {{ unsplitTransaction(transactionId: "{tx}") {{ splits {{ index }} }} }}"#);
    let unsplit_response = schema
        .execute(authed(Request::new(unsplit_query), Some(caller)))
        .await;
    assert!(unsplit_response.errors.is_empty(), "{:?}", unsplit_response.errors);
    let unsplit_data = unsplit_response.data.into_json().unwrap();
    assert_eq!(
        unsplit_data["unsplitTransaction"]["splits"].as_array().unwrap().len(),
        0
    );
}

#[tokio::test]
async fn rule_crud_and_reapply_scope_to_the_callers_accounts() {
    let (_kafka, bootstrap) = start_kafka_with_labeling_topics().await;
    let db = common::db().await;

    // Rule mutations and `rules` are admin-only.
    let (user, _) = common::seed_admin(&db, "grace-rule-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Grace's account").await;
    common::link(&db, user, account).await;
    let (_, subs_slug) = common::seed_category(&db, "expense.subscriptions", "Subscriptions", "EXPENSE").await;
    common::seed_transaction(&db, account, "2024-04-03", "-9.99", Some("Streamflix")).await;

    let schema = create_schema(db.clone(), settings_with_kafka(&bootstrap));
    let caller = AuthenticatedUser {
        user_id: user,
        username: "grace-rule-test".to_string(),
        account_ids: vec![account],
    };

    let create_query = format!(
        r#"mutation {{
        createRule(input: {{
            name: "Streamflix subscription"
            categorySlug: "{subs_slug}"
            conditions: {{ description_contains: "Streamflix" }}
            priority: 10
        }}) {{ id name state priority }}
    }}"#
    );
    let create_response = schema
        .execute(authed(Request::new(create_query), Some(caller.clone())))
        .await;
    assert!(create_response.errors.is_empty(), "{:?}", create_response.errors);
    let create_data = create_response.data.into_json().unwrap();
    let rule_id = create_data["createRule"]["id"].as_str().unwrap().to_string();
    assert_eq!(create_data["createRule"]["state"], "ACTIVE");

    // `rules` (unscoped, category tree / rules are tenant-agnostic this
    // iteration) must see the rule right after publish-then-upsert.
    let list_response = schema
        .execute(authed(Request::new("{ rules { id name } }"), Some(caller.clone())))
        .await;
    assert!(list_response.errors.is_empty(), "{:?}", list_response.errors);
    let list_data = list_response.data.into_json().unwrap();
    assert!(list_data["rules"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == rule_id));

    // `setRuleState` revokes it.
    let revoke_query = format!(r#"mutation {{ setRuleState(id: "{rule_id}", state: REVOKED) {{ state }} }}"#);
    let revoke_response = schema
        .execute(authed(Request::new(revoke_query), Some(caller.clone())))
        .await;
    assert!(revoke_response.errors.is_empty(), "{:?}", revoke_response.errors);
    assert_eq!(
        revoke_response.data.into_json().unwrap()["setRuleState"]["state"],
        "REVOKED"
    );

    // `reapplyRule` is scoped: it must not error for the owning caller and
    // returns the (possibly zero) count of matching transactions in their
    // accounts, publishing a `label-request` behind the scenes.
    let reapply_query = format!(r#"mutation {{ reapplyRule(id: "{rule_id}") }}"#);
    let reapply_response = schema
        .execute(authed(Request::new(reapply_query), Some(caller)))
        .await;
    assert!(reapply_response.errors.is_empty(), "{:?}", reapply_response.errors);
}

#[tokio::test]
async fn category_breakdown_reconciles_labelled_and_unlabelled_totals() {
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "heidi-breakdown-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Heidi's account").await;
    common::link(&db, user, account).await;

    let (groceries, groceries_slug) =
        common::seed_category(&db, "expense.groceries3", "Groceries", "EXPENSE").await;
    let tx_labelled = common::seed_transaction(&db, account, "2024-05-01", "-40.00", Some("Shop")).await;
    common::seed_transaction_label(&db, tx_labelled, Some(groceries), "user", "resolved").await;
    // No `transaction_label` row at all: falls into the `uncategorized` bucket.
    common::seed_transaction(&db, account, "2024-05-02", "-15.00", Some("Unknown shop")).await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "heidi-breakdown-test".to_string(),
        account_ids: vec![account],
    };

    let query = r#"{
        categoryBreakdown(filter: { startDate: "2024-05-01", endDate: "2024-05-02" }, level: 1) {
            rows { category { slug } amount }
            uncategorized { amount }
            currency
        }
    }"#;
    let response = schema.execute(authed(Request::new(query), Some(caller))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let breakdown = &data["categoryBreakdown"];
    let rows = breakdown["rows"].as_array().unwrap();

    let rows_total: Decimal = rows
        .iter()
        .map(|r| Decimal::from_str(r["amount"].as_str().unwrap()).unwrap())
        .sum();
    assert_eq!(rows_total, Decimal::from_str("40.0000").unwrap());
    assert!(rows.iter().any(|r| r["category"]["slug"] == groceries_slug));
    assert_eq!(
        Decimal::from_str(breakdown["uncategorized"]["amount"].as_str().unwrap()).unwrap(),
        Decimal::from_str("15.0000").unwrap()
    );
}

#[tokio::test]
async fn review_queue_only_surfaces_the_callers_own_needs_review_transactions() {
    let db = common::db().await;

    let (user_a, _) = common::seed_user(&db, "ivan-review-test", "pw").await;
    let account_a = common::seed_account(&db, "EUR", "Ivan's account").await;
    common::link(&db, user_a, account_a).await;
    let (_user_b, _) = common::seed_user(&db, "judy-review-test", "pw").await;
    let account_b = common::seed_account(&db, "EUR", "Judy's account").await;

    let tx_a = common::seed_transaction(&db, account_a, "2024-05-10", "-20.00", Some("Mystery")).await;
    common::seed_transaction_label(&db, tx_a, None, "llm", "needs_review").await;
    let tx_b = common::seed_transaction(&db, account_b, "2024-05-10", "-20.00", Some("Mystery")).await;
    common::seed_transaction_label(&db, tx_b, None, "llm", "needs_review").await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user_a,
        username: "ivan-review-test".to_string(),
        account_ids: vec![account_a],
    };

    let query = "{ reviewQueue { totalCount transactions { id } } }";
    let response = schema.execute(authed(Request::new(query), Some(caller))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    assert_eq!(data["reviewQueue"]["totalCount"], 1);
    assert_eq!(data["reviewQueue"]["transactions"][0]["id"], tx_a.to_string());
}

#[tokio::test]
async fn cashflow_graph_category_dimension_conserves_flow_and_links_to_labelled_categories() {
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "kyle-category-sankey-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Kyle's account").await;
    common::link(&db, user, account).await;

    let (rent_id, rent_slug) = common::seed_category(&db, "expense.rent", "Rent", "EXPENSE").await;
    common::seed_transaction(&db, account, "2024-06-01", "1000.00", Some("Employer")).await;
    let tx_rent = common::seed_transaction(&db, account, "2024-06-02", "-500.00", Some("Landlord")).await;
    common::seed_transaction_label(&db, tx_rent, Some(rent_id), "user", "resolved").await;
    // Unlabelled spending: falls into the category mode's "Uncategorized" bucket.
    common::seed_transaction(&db, account, "2024-06-03", "-50.00", Some("Unknown shop")).await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "kyle-category-sankey-test".to_string(),
        account_ids: vec![account],
    };

    let query = r#"{ cashflowGraph(
        filter: { startDate: "2024-06-01", endDate: "2024-06-03" }
        grouping: { dimensions: [INCOME_SOURCE, ACCOUNT, CATEGORY] }
    ) {
        nodes { id kind value refType refId }
        links { sourceId targetId value }
    } }"#;
    let response = schema.execute(authed(Request::new(query), Some(caller))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let nodes = data["cashflowGraph"]["nodes"].as_array().unwrap();
    let links = data["cashflowGraph"]["links"].as_array().unwrap();

    // Flow conservation, same invariant as the OUTCOME-dimension test above.
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
        assert_eq!(flow, reported, "node {id} does not conserve flow");
    }

    let rent_node = nodes
        .iter()
        .find(|n| n["refType"] == "category" && n["refId"] == rent_slug)
        .unwrap_or_else(|| panic!("no category node for '{rent_slug}' among {nodes:?}"));
    assert_eq!(
        Decimal::from_str(rent_node["value"].as_str().unwrap()).unwrap(),
        Decimal::from_str("500.0000").unwrap()
    );
    assert!(nodes.iter().any(|n| n["kind"] == "CATEGORY"));
}

/// `uncategorized` means "no category assigned", not "no label row". A
/// transaction labelled while the taxonomy was empty has a row with a NULL
/// `category_id`: it renders as "—" and must be findable. The old predicate
/// tested for the absence of the row itself, so on real data — where every
/// transaction has a row — filtering for Uncategorized returned nothing while
/// the category breakdown reported a large uncategorised total.
#[tokio::test]
async fn uncategorized_filter_means_no_category_not_no_label_row() {
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "uncat-filter-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Uncategorized filter account").await;
    common::link(&db, user, account).await;
    let (groceries, _) = common::seed_category(&db, "groceries", "Groceries", "expense").await;

    // No label row at all.
    let no_row = common::seed_transaction(&db, account, "2024-08-01", "-10.00", Some("No row")).await;
    // A label row whose category is NULL — the case that was invisible.
    let null_cat = common::seed_transaction(&db, account, "2024-08-02", "-20.00", Some("Null cat")).await;
    common::seed_transaction_label(&db, null_cat, None, "llm", "resolved").await;
    // Properly categorised.
    let categorised =
        common::seed_transaction(&db, account, "2024-08-03", "-30.00", Some("Categorised")).await;
    common::seed_transaction_label(&db, categorised, Some(groceries), "llm", "resolved").await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "uncat-filter-test".to_string(),
        account_ids: vec![account],
    };

    let ids_for = |uncategorized: bool, caller: AuthenticatedUser| {
        let schema = schema.clone();
        async move {
            let query = format!(
                r#"{{ transactions(filter: {{ startDate: "2024-08-01", endDate: "2024-08-03",
                     uncategorized: {uncategorized} }}) {{ items {{ id }} }} }}"#
            );
            let response = schema.execute(authed(Request::new(query), Some(caller))).await;
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            let data = response.data.into_json().unwrap();
            data["transactions"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["id"].as_str().unwrap().to_string())
                .collect::<Vec<String>>()
        }
    };

    let mut uncategorised = ids_for(true, caller.clone()).await;
    uncategorised.sort();
    let mut expected = vec![no_row.to_string(), null_cat.to_string()];
    expected.sort();
    assert_eq!(
        uncategorised, expected,
        "both the missing row and the NULL category must count as uncategorised"
    );

    let categorised_ids = ids_for(false, caller).await;
    assert_eq!(
        categorised_ids,
        vec![categorised.to_string()],
        "uncategorized: false must select only rows that really have a category"
    );
}

/// A refund must cancel the charge it reverses, not add to it. The breakdown
/// summed `amount.abs()` per contribution, so a 1000 medical bill reimbursed
/// in full read as 2000 spent instead of 0 — and it contradicted iteration 4's
/// goal evaluation (§3.3), where a refund inside the scope reduces the total,
/// so a goal and this breakdown disagreed about the same two transactions.
#[tokio::test]
async fn a_refund_nets_against_the_charge_in_the_category_breakdown() {
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "refund-net-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Refund netting account").await;
    common::link(&db, user, account).await;
    let (health, _) = common::seed_category(&db, "health", "Health", "expense").await;

    let bill = common::seed_transaction(&db, account, "2024-09-10", "-1000.00", Some("Praxis")).await;
    common::seed_transaction_label(&db, bill, Some(health), "user", "resolved").await;
    let refund =
        common::seed_transaction(&db, account, "2024-09-20", "1000.00", Some("Krankenkasse")).await;
    common::seed_transaction_label(&db, refund, Some(health), "user", "resolved").await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "refund-net-test".to_string(),
        account_ids: vec![account],
    };

    let response = schema
        .execute(authed(
            Request::new(
                r#"{ categoryBreakdown(filter: { startDate: "2024-09-01", endDate: "2024-09-30" },
                      level: 1) { rows { category { slug } amount transactionCount } } }"#,
            ),
            Some(caller),
        ))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let rows = data["categoryBreakdown"]["rows"].as_array().unwrap();
    // `seed_category` suffixes the slug to keep tests independent.
    let row = rows
        .iter()
        .find(|r| r["category"]["slug"].as_str().unwrap().starts_with("health"))
        .unwrap_or_else(|| panic!("expected a health row among {rows:?}"));

    assert_eq!(
        Decimal::from_str(row["amount"].as_str().unwrap()).unwrap(),
        Decimal::ZERO,
        "a fully reimbursed bill nets to zero, not to twice the amount"
    );
    assert_eq!(row["transactionCount"], 2, "both transactions still counted");
}

/// The SDL documents `search` as a case-insensitive substring, and the
/// hand-rolled cashflow SQL has always used ILIKE — but the transaction list
/// used LIKE, so the charts and the list disagreed about the same term.
#[tokio::test]
async fn the_search_filter_is_case_insensitive() {
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "search-case-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Search casing account").await;
    common::link(&db, user, account).await;

    let tx =
        common::seed_transaction(&db, account, "2024-09-05", "-9.99", Some("SPOTIFY AB")).await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "search-case-test".to_string(),
        account_ids: vec![account],
    };

    for term in ["spotify", "SPOTIFY", "Spotify", "potify"] {
        let query = format!(
            r#"{{ transactions(filter: {{ startDate: "2024-09-01", endDate: "2024-09-30",
                 search: "{term}" }}) {{ items {{ id }} }} }}"#
        );
        let response = schema
            .execute(authed(Request::new(query), Some(caller.clone())))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        let ids: Vec<&str> = data["transactions"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert!(
            ids.contains(&tx.to_string().as_str()),
            "searching {term:?} must match \"SPOTIFY AB\""
        );
    }
}

// ---------------------------------------------------------------------------
// Display aliases
// ---------------------------------------------------------------------------

mod display_aliases {
    use super::*;
    use chrono::Utc;
    use entity::entities::transaction;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
    use secrecy::ExposeSecret;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;
    use webapp::kafka::labeling::{AliasKind, DisplayAliasRecord, CURRENT_SCHEMA_VERSION};
    use webapp::projection::display_alias::project_display_alias;

    struct Tenant {
        user: AuthenticatedUser,
        account: Uuid,
    }

    async fn tenant(db: &DatabaseConnection, name: &str) -> Tenant {
        let (user_id, username) = common::seed_user(db, name, "pw").await;
        let account = common::seed_account(db, "EUR", "Pavlo").await;
        common::link(db, user_id, account).await;
        Tenant { user: AuthenticatedUser { user_id, username, account_ids: vec![account] }, account }
    }

    async fn tx(db: &DatabaseConnection, account: Uuid, name: &str) -> Uuid {
        let id = common::seed_transaction(db, account, "2024-07-01", "-5", Some(name)).await;
        let key = webapp::labeling::normalize::normalize(Some(name), None);
        let mut model: transaction::ActiveModel =
            transaction::Entity::find_by_id(id).one(db).await.unwrap().unwrap().into();
        model.counterparty_key = Set(Some(key));
        model.update(db).await.unwrap();
        id
    }

    async fn alias(db: &DatabaseConnection, user: Uuid, kind: AliasKind, key: &str, alias: &str) {
        let record = DisplayAliasRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            user_id: user,
            kind,
            key: key.to_string(),
            alias: alias.to_string(),
            revision: Utc::now(),
        };
        project_display_alias(db, user, kind, key, Some(record)).await.unwrap();
    }

    async fn run(
        schema: &webapp::graphql::AppSchema,
        user: &AuthenticatedUser,
        query: &str,
    ) -> serde_json::Value {
        let response = schema.execute(authed(Request::new(query), Some(user.clone()))).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        response.data.into_json().unwrap()
    }

    const ITEMS: &str = r#"{ transactions(filter: { startDate: "2024-07-01", endDate: "2024-07-31" })
        { items { counterpartyName counterpartyDisplayName } } }"#;

    #[tokio::test]
    async fn the_alias_covers_every_spelling_and_leaves_the_raw_name_alone() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let t = tenant(&db, &format!("alias-{}", Uuid::new_v4().simple())).await;
        let unique = Uuid::new_v4().simple().to_string();
        let spellings = [
            format!("Mum {unique} GmbH"),
            format!("MUM {unique}"),
            format!("mum {unique} 12.03"),
        ];
        for s in &spellings {
            tx(&db, t.account, s).await;
        }
        tx(&db, t.account, "Unrelated Shop").await;
        let key = webapp::labeling::normalize::normalize(Some(&spellings[0]), None);
        alias(&db, t.user.user_id, AliasKind::Counterparty, &key, "Mum").await;

        let data = run(&schema, &t.user, ITEMS).await;
        for item in data["transactions"]["items"].as_array().unwrap() {
            let raw = item["counterpartyName"].as_str().unwrap();
            let shown = item["counterpartyDisplayName"].as_str().unwrap();
            if raw == "Unrelated Shop" {
                assert_eq!(shown, raw, "an unaliased merchant shows the bank name");
            } else {
                assert_eq!(shown, "Mum", "{raw:?} must show the alias");
            }
        }

        // A tombstone brings the bank name back, unchanged.
        project_display_alias(db.as_ref(), t.user.user_id, AliasKind::Counterparty, &key, None)
            .await
            .unwrap();
        let data = run(&schema, &t.user, ITEMS).await;
        for item in data["transactions"]["items"].as_array().unwrap() {
            assert_eq!(item["counterpartyName"], item["counterpartyDisplayName"]);
        }
    }

    #[tokio::test]
    async fn aliases_are_per_user_and_account_aliases_need_an_accessible_account() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let alice = tenant(&db, &format!("alias-a-{}", Uuid::new_v4().simple())).await;
        let bob = tenant(&db, &format!("alias-b-{}", Uuid::new_v4().simple())).await;
        let name = format!("Shared Shop {}", Uuid::new_v4().simple());
        tx(&db, alice.account, &name).await;
        tx(&db, bob.account, &name).await;
        let key = webapp::labeling::normalize::normalize(Some(&name), None);
        alias(&db, alice.user.user_id, AliasKind::Counterparty, &key, "Alice's shop").await;
        alias(&db, alice.user.user_id, AliasKind::Account, &alice.account.to_string(), "Alice main").await;

        let bobs = run(&schema, &bob.user, ITEMS).await;
        assert_eq!(bobs["transactions"]["items"][0]["counterpartyDisplayName"], name);
        let list = run(&schema, &bob.user, "{ displayAliases { key } }").await;
        assert_eq!(list["displayAliases"].as_array().unwrap().len(), 0);

        let alices = run(
            &schema,
            &alice.user,
            "{ displayAliases { kind key alias rawName transactionCount } accounts { displayName label } }",
        )
        .await;
        assert_eq!(alices["displayAliases"].as_array().unwrap().len(), 2);
        let merchant = alices["displayAliases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["kind"] == "COUNTERPARTY")
            .unwrap();
        assert_eq!(merchant["rawName"], name);
        assert_eq!(merchant["transactionCount"], 1);
        assert_eq!(alices["accounts"][0]["displayName"], "Alice main");
        assert_eq!(alices["accounts"][0]["label"], "Pavlo", "label stays the login label");
        let bob_accounts = run(&schema, &bob.user, "{ accounts { displayName } }").await;
        assert_eq!(bob_accounts["accounts"][0]["displayName"], "Pavlo");

        // Bob cannot alias Alice's account.
        let mutation = format!(
            r#"mutation {{ setDisplayAlias(kind: ACCOUNT, key: "{}", alias: "mine") {{ key }} }}"#,
            alice.account
        );
        let response = schema.execute(authed(Request::new(mutation), Some(bob.user.clone()))).await;
        assert!(response.errors[0].message.contains("not accessible"), "{:?}", response.errors);
    }

    /// Ids the list returns for `search`, plus what `cashflowSummary` and
    /// `cashflowGraph` count for the same term - the three must agree.
    async fn search_everywhere(
        schema: &webapp::graphql::AppSchema,
        user: &AuthenticatedUser,
        search: &str,
    ) -> (Vec<String>, i64, bool) {
        let filter = format!(r#"{{ startDate: "2024-07-01", endDate: "2024-07-31", search: "{search}" }}"#);
        let list = run(
            schema,
            user,
            &format!("{{ transactions(filter: {filter}) {{ items {{ id }} }} }}"),
        )
        .await;
        let mut ids: Vec<String> = list["transactions"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect();
        ids.sort();
        let summary = run(
            schema,
            user,
            &format!(
                "{{ cashflowSummary(filter: {filter}, granularity: MONTH) {{ total {{ transactionCount }} }} }}"
            ),
        )
        .await;
        let graph = run(
            schema,
            user,
            &format!("{{ cashflowGraph(filter: {filter}) {{ links {{ value }} }} }}"),
        )
        .await;
        (
            ids,
            summary["cashflowSummary"]["total"]["transactionCount"].as_i64().unwrap(),
            !graph["cashflowGraph"]["links"].as_array().unwrap().is_empty(),
        )
    }

    #[tokio::test]
    async fn search_finds_the_nickname_and_the_bank_name_and_only_the_callers_nickname() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let alice = tenant(&db, &format!("alias-s-a-{}", Uuid::new_v4().simple())).await;
        let bob = tenant(&db, &format!("alias-s-b-{}", Uuid::new_v4().simple())).await;
        let u = Uuid::new_v4().simple().to_string();
        let bank = format!("Backwerk Filiale {u}");
        let mine = tx(&db, alice.account, &bank).await;
        let bobs = tx(&db, bob.account, &bank).await;
        tx(&db, alice.account, &format!("Unrelated {u}")).await;
        let key = webapp::labeling::normalize::normalize(Some(&bank), None);
        let nickname = format!("Bakery{u}");
        alias(&db, alice.user.user_id, AliasKind::Counterparty, &key, &nickname).await;

        // The nickname finds the row - case-insensitively, as a substring -
        // in the list, the summary and the graph alike.
        for term in [nickname.clone(), nickname.to_lowercase(), format!("akery{u}")] {
            let (ids, summary, graph) = search_everywhere(&schema, &alice.user, &term).await;
            assert_eq!(ids, vec![mine.to_string()], "searching the nickname {term:?}");
            assert_eq!(summary, 1, "cashflowSummary must agree with the list for {term:?}");
            assert!(graph, "cashflowGraph must see the match the list sees for {term:?}");
        }
        // The bank's wording still finds it.
        let (ids, summary, graph) = search_everywhere(&schema, &alice.user, &format!("Backwerk Filiale {u}")).await;
        assert_eq!(ids, vec![mine.to_string()]);
        assert_eq!((summary, graph), (1, true));

        // Bob shares the merchant key but not Alice's alias: it matches
        // nothing for him anywhere, and his own row is still found by name.
        let (ids, summary, graph) = search_everywhere(&schema, &bob.user, &nickname).await;
        assert!(ids.is_empty(), "another user's alias must not match: {ids:?}");
        assert_eq!((summary, graph), (0, false));
        let (ids, summary, _) = search_everywhere(&schema, &bob.user, &format!("Backwerk Filiale {u}")).await;
        assert_eq!(ids, vec![bobs.to_string()]);
        assert_eq!(summary, 1);

        // Removing the alias takes the nickname out of search again.
        project_display_alias(db.as_ref(), alice.user.user_id, AliasKind::Counterparty, &key, None)
            .await
            .unwrap();
        let (ids, summary, _) = search_everywhere(&schema, &alice.user, &nickname).await;
        assert!(ids.is_empty());
        assert_eq!(summary, 0);
    }

    #[tokio::test]
    async fn counterparty_sort_follows_the_displayed_name_with_aliased_and_plain_rows_interleaved() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let alice = tenant(&db, &format!("alias-o-a-{}", Uuid::new_v4().simple())).await;
        let bob = tenant(&db, &format!("alias-o-b-{}", Uuid::new_v4().simple())).await;
        // Raw order: Aldi < Edeka < Penny < Zalando. Alice's displayed order:
        // Aldi, Bakery (Zalando), Edeka, mum (Penny) - the lower-case "mum"
        // sorts after "Edeka" because the sort is case-insensitive.
        let aldi = tx(&db, alice.account, "Aldi").await;
        let edeka = tx(&db, alice.account, "Edeka").await;
        let penny = tx(&db, alice.account, "Penny").await;
        let zalando = tx(&db, alice.account, "Zalando").await;
        let b_zalando = tx(&db, bob.account, "Zalando").await;
        let b_aldi = tx(&db, bob.account, "Aldi").await;
        alias(&db, alice.user.user_id, AliasKind::Counterparty, "zalando", "Bakery").await;
        alias(&db, alice.user.user_id, AliasKind::Counterparty, "penny", "mum").await;

        let order = |user: AuthenticatedUser, dir: &'static str| {
            let schema = schema.clone();
            async move {
                let q = format!(
                    r#"{{ transactions(filter: {{ startDate: "2024-07-01", endDate: "2024-07-31" }},
                         sort: {{ field: COUNTERPARTY_NAME, direction: {dir} }}) {{ items {{ id }} }} }}"#
                );
                run(&schema, &user, &q).await["transactions"]["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i["id"].as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            }
        };
        let s = |ids: &[Uuid]| ids.iter().map(Uuid::to_string).collect::<Vec<_>>();
        assert_eq!(order(alice.user.clone(), "ASC").await, s(&[aldi, zalando, edeka, penny]));
        assert_eq!(order(alice.user.clone(), "DESC").await, s(&[penny, edeka, zalando, aldi]));
        // Alice's nicknames do not reorder Bob's rows.
        assert_eq!(order(bob.user.clone(), "ASC").await, s(&[b_aldi, b_zalando]));
    }

    #[tokio::test]
    async fn a_page_of_rows_issues_one_alias_query() {
        let db = common::db().await;
        let t = tenant(&db, &format!("alias-q-{}", Uuid::new_v4().simple())).await;
        for i in 0..25 {
            tx(&db, t.account, &format!("Row Shop {i} {}", Uuid::new_v4().simple())).await;
        }
        alias(&db, t.user.user_id, AliasKind::Counterparty, "row shop", "Rows").await;

        // A private connection with a statement counter in front of the schema.
        let url = common::dummy_settings().database_url.as_ref().unwrap().expose_secret().to_string();
        let mut counted = sea_orm::Database::connect(url).await.unwrap();
        let alias_queries = Arc::new(AtomicUsize::new(0));
        let seen = alias_queries.clone();
        counted.set_metric_callback(move |info| {
            if info.statement.sql.contains("\"display_alias\"") {
                seen.fetch_add(1, Ordering::SeqCst);
            }
        });
        let schema = create_schema(Arc::new(counted), common::dummy_settings());

        let data = run(&schema, &t.user, ITEMS).await;
        assert!(data["transactions"]["items"].as_array().unwrap().len() >= 25);
        assert_eq!(
            alias_queries.load(Ordering::SeqCst),
            1,
            "one alias query for the whole page, not one per row"
        );
    }
}

// ---------------------------------------------------------------------------
// Savings accounts: the per-user flag (storage + write path) and savingsSummary
// ---------------------------------------------------------------------------

mod savings {
    use super::*;
    use chrono::Utc;
    use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, Statement};
    use uuid::Uuid;
    use webapp::kafka::savings::{AccountSavingsRecord, CURRENT_SCHEMA_VERSION};
    use webapp::projection::account_savings::project_account_savings;

    /// A user with two accounts they may flag.
    struct Tenant {
        user: AuthenticatedUser,
        s1: Uuid,
        s2: Uuid,
    }

    async fn tenant(db: &DatabaseConnection, name: &str) -> Tenant {
        let (user_id, username) = common::seed_user(db, name, "pw").await;
        let mut ids = Vec::new();
        for label in ["Tagesgeld", "Depot"] {
            let id = common::seed_account(db, "EUR", label).await;
            common::link(db, user_id, id).await;
            ids.push(id);
        }
        Tenant {
            user: AuthenticatedUser { user_id, username, account_ids: ids.clone() },
            s1: ids[0],
            s2: ids[1],
        }
    }

    fn record(user: Uuid, account: Uuid, revision: chrono::DateTime<Utc>) -> AccountSavingsRecord {
        AccountSavingsRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            user_id: user,
            account_id: account,
            revision,
        }
    }

    async fn flag(db: &DatabaseConnection, user: Uuid, account: Uuid, on: bool) {
        let rec = on.then(|| record(user, account, Utc::now()));
        project_account_savings(db, user, account, rec).await.unwrap();
    }

    /// `isSavings` for every account of `user`, keyed by account id.
    async fn flags(
        schema: &webapp::graphql::AppSchema,
        user: &AuthenticatedUser,
    ) -> std::collections::HashMap<String, bool> {
        let response = schema
            .execute(authed(Request::new("{ accounts { id isSavings } }"), Some(user.clone())))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        data["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| (a["id"].as_str().unwrap().to_string(), a["isSavings"].as_bool().unwrap()))
            .collect()
    }

    #[tokio::test]
    async fn the_flag_is_off_until_set_follows_the_projection_and_a_tombstone_clears_it() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let t = tenant(&db, &format!("sv-flag-{}", Uuid::new_v4().simple())).await;
        let (s1, s2) = (t.s1.to_string(), t.s2.to_string());

        let before = flags(&schema, &t.user).await;
        assert_eq!((before[&s1], before[&s2]), (false, false));

        flag(&db, t.user.user_id, t.s1, true).await;
        let on = flags(&schema, &t.user).await;
        assert_eq!((on[&s1], on[&s2]), (true, false));

        flag(&db, t.user.user_id, t.s1, false).await;
        let off = flags(&schema, &t.user).await;
        assert_eq!((off[&s1], off[&s2]), (false, false), "a tombstone clears the flag");
    }

    #[tokio::test]
    async fn replaying_the_records_leaves_the_same_flags() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let t = tenant(&db, &format!("sv-replay-{}", Uuid::new_v4().simple())).await;
        let u = t.user.user_id;
        let now = Utc::now();
        // Log order: s1 on, s2 on, s1 off (tombstone).
        let replay = || async {
            project_account_savings(db.as_ref(), u, t.s1, Some(record(u, t.s1, now))).await.unwrap();
            project_account_savings(db.as_ref(), u, t.s2, Some(record(u, t.s2, now))).await.unwrap();
            project_account_savings(db.as_ref(), u, t.s1, None).await.unwrap();
        };
        replay().await;
        let first = flags(&schema, &t.user).await;
        replay().await;
        let second = flags(&schema, &t.user).await;
        assert_eq!(first, second);
        assert!(!second[&t.s1.to_string()]);
        assert!(second[&t.s2.to_string()]);
    }

    #[tokio::test]
    async fn one_users_flag_never_shows_on_anothers_accounts() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let alice = tenant(&db, &format!("sv-iso-a-{}", Uuid::new_v4().simple())).await;
        let bob = tenant(&db, &format!("sv-iso-b-{}", Uuid::new_v4().simple())).await;
        // Both see the same shared account; only Alice flags it.
        common::link(&db, bob.user.user_id, alice.s1).await;
        let bob_user = AuthenticatedUser {
            account_ids: [bob.user.account_ids.clone(), vec![alice.s1]].concat(),
            ..bob.user.clone()
        };
        flag(&db, alice.user.user_id, alice.s1, true).await;

        assert!(flags(&schema, &alice.user).await[&alice.s1.to_string()]);
        let bobs = flags(&schema, &bob_user).await;
        assert!(bobs.values().all(|on| !on), "Bob flagged nothing: {bobs:?}");
    }

    #[tokio::test]
    async fn set_account_savings_rejects_someone_elses_account() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let alice = tenant(&db, &format!("sv-rej-a-{}", Uuid::new_v4().simple())).await;
        let bob = tenant(&db, &format!("sv-rej-b-{}", Uuid::new_v4().simple())).await;
        let mutation = format!(
            r#"mutation {{ setAccountSavings(accountId: "{}", isSavings: true) {{ id }} }}"#,
            alice.s1
        );
        let response = schema.execute(authed(Request::new(mutation), Some(bob.user.clone()))).await;
        assert!(response.errors[0].message.contains("not accessible"), "{:?}", response.errors);
        assert!(flags(&schema, &alice.user).await.values().all(|on| !on));
    }

    #[tokio::test]
    async fn a_stale_flag_record_does_not_undo_a_newer_one() {
        let db = common::db().await;
        let t = tenant(&db, &format!("sv-stale-{}", Uuid::new_v4().simple())).await;
        let u = t.user.user_id;
        let newer = Utc::now();
        let older = newer - chrono::Duration::seconds(60);
        project_account_savings(db.as_ref(), u, t.s1, Some(record(u, t.s1, newer))).await.unwrap();
        project_account_savings(db.as_ref(), u, t.s1, Some(record(u, t.s1, older))).await.unwrap();
        let row = entity::entities::account_savings::Entity::find_by_id((u, t.s1))
            .one(db.as_ref())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.revision.timestamp_micros(), newer.timestamp_micros());
    }

    // -- savingsSummary ------------------------------------------------------

    use rust_decimal::Decimal;
    use sea_orm::{ActiveModelTrait, Set};
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    /// Marks two transactions as the two legs of one detected transfer.
    async fn pair_as_transfer(db: &DatabaseConnection, a: Uuid, b: Uuid) {
        for (id, other) in [(a, b), (b, a)] {
            entity::entities::transaction_insight::ActiveModel {
                transaction_id: Set(id),
                is_transfer: Set(true),
                transfer_counterpart_id: Set(Some(other)),
                transfer_match: Set(Some("amount_date".to_string())),
                is_recurring: Set(false),
                recurring_series_id: Set(None),
                recurring_cadence: Set(None),
                recurring_median_amount: Set(None),
                detected_at: Set(Utc::now().into()),
                revision: Set(Utc::now().into()),
            }
            .insert(db)
            .await
            .unwrap();
        }
    }

    /// An extra, ordinary (never flagged) account of the same user.
    async fn checking(db: &DatabaseConnection, t: &mut Tenant) -> Uuid {
        let id = common::seed_account(db, "EUR", "Giro").await;
        common::link(db, t.user.user_id, id).await;
        t.user.account_ids.push(id);
        id
    }

    /// `savingsSummary(filter: {<filter>})`, as JSON (`null` when absent).
    async fn summary(
        schema: &webapp::graphql::AppSchema,
        user: &AuthenticatedUser,
        filter: &str,
    ) -> serde_json::Value {
        let query = format!(
            "{{ savingsSummary(filter: {{ {filter} }}) {{ currency netPutAside paidIn withdrawn \
             internalTransferCount spentDirectly accounts {{ account {{ id }} paidIn withdrawn net }} }} }}"
        );
        let response = schema.execute(authed(Request::new(query), Some(user.clone()))).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        response.data.into_json().unwrap()["savingsSummary"].clone()
    }

    fn money(v: &serde_json::Value, key: &str) -> Decimal {
        dec(v[key].as_str().unwrap_or_else(|| panic!("{key} missing in {v}")))
    }

    const YEAR: &str = r#"startDate: "2024-01-01", endDate: "2024-12-31""#;

    #[tokio::test]
    async fn savings_summary_is_null_until_an_account_is_marked_then_reflects_it() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let t = tenant(&db, &format!("sv-sum-{}", Uuid::new_v4().simple())).await;
        common::seed_transaction(&db, t.s1, "2024-03-01", "100", Some("Employer")).await;
        common::seed_transaction(&db, t.s1, "2024-03-05", "-30", Some("Shop")).await;

        assert!(summary(&schema, &t.user, YEAR).await.is_null(), "no account marked");

        flag(&db, t.user.user_id, t.s1, true).await;
        let s = summary(&schema, &t.user, YEAR).await;
        assert_eq!(s["currency"], "EUR");
        assert_eq!(
            (money(&s, "paidIn"), money(&s, "withdrawn"), money(&s, "netPutAside")),
            (dec("100"), dec("30"), dec("70"))
        );
        assert_eq!(money(&s, "spentDirectly"), dec("30"), "a payment to a shop is also spending");
        assert_eq!(s["internalTransferCount"], 0);
        assert_eq!(s["accounts"].as_array().unwrap().len(), 1);

        flag(&db, t.user.user_id, t.s1, false).await;
        assert!(summary(&schema, &t.user, YEAR).await.is_null(), "unmarking returns to null");
    }

    #[tokio::test]
    async fn a_savings_to_savings_transfer_nets_to_zero_and_is_counted_once() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let mut t = tenant(&db, &format!("sv-int-{}", Uuid::new_v4().simple())).await;
        let giro = checking(&db, &mut t).await;
        flag(&db, t.user.user_id, t.s1, true).await;
        flag(&db, t.user.user_id, t.s2, true).await;

        // 50 moves s1 -> s2 (both legs are rows, both flagged by the detector).
        let out = common::seed_transaction(&db, t.s1, "2024-03-02", "-50", Some("Own account")).await;
        let into = common::seed_transaction(&db, t.s2, "2024-03-02", "50", Some("Own account")).await;
        pair_as_transfer(&db, out, into).await;
        // 200 comes in from the checking account: a real deposit, counted once.
        let from_giro = common::seed_transaction(&db, giro, "2024-03-03", "-200", Some("Own account")).await;
        let to_s1 = common::seed_transaction(&db, t.s1, "2024-03-03", "200", Some("Own account")).await;
        pair_as_transfer(&db, from_giro, to_s1).await;
        // 20 goes back to checking: a withdrawal, but not spending.
        let from_s2 = common::seed_transaction(&db, t.s2, "2024-03-04", "-20", Some("Own account")).await;
        let to_giro = common::seed_transaction(&db, giro, "2024-03-04", "20", Some("Own account")).await;
        pair_as_transfer(&db, from_s2, to_giro).await;

        let s = summary(&schema, &t.user, YEAR).await;
        assert_eq!(
            (money(&s, "paidIn"), money(&s, "withdrawn"), money(&s, "netPutAside")),
            (dec("200"), dec("20"), dec("180"))
        );
        assert_eq!(s["internalTransferCount"], 1, "two rows, one movement");
        assert_eq!(money(&s, "spentDirectly"), dec("0"), "transfers to own accounts are not spending");
        let per_account: Vec<Decimal> = s["accounts"].as_array().unwrap().iter().map(|a| money(a, "net")).collect();
        assert_eq!(per_account.iter().sum::<Decimal>(), dec("180"));
    }

    #[tokio::test]
    async fn savings_summary_honours_the_period_and_the_panel_filter() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let t = tenant(&db, &format!("sv-flt-{}", Uuid::new_v4().simple())).await;
        flag(&db, t.user.user_id, t.s1, true).await;
        flag(&db, t.user.user_id, t.s2, true).await;
        common::seed_transaction(&db, t.s1, "2024-01-10", "100", Some("Employer")).await;
        common::seed_transaction(&db, t.s1, "2024-06-10", "300", Some("Employer")).await;
        common::seed_transaction(&db, t.s2, "2024-06-11", "7", Some("Bank interest")).await;
        // A leg of an s1 -> s2 movement lands in June; its counterpart is on s2 in July.
        let out = common::seed_transaction(&db, t.s1, "2024-06-30", "-40", Some("Own account")).await;
        let into = common::seed_transaction(&db, t.s2, "2024-07-01", "40", Some("Own account")).await;
        pair_as_transfer(&db, out, into).await;

        let jan = summary(&schema, &t.user, r#"startDate: "2024-01-01", endDate: "2024-01-31""#).await;
        assert_eq!(money(&jan, "paidIn"), dec("100"));
        let june = summary(&schema, &t.user, r#"startDate: "2024-06-01", endDate: "2024-06-30""#).await;
        assert_eq!((money(&june, "paidIn"), money(&june, "withdrawn")), (dec("307"), dec("0")));
        assert_eq!(june["internalTransferCount"], 1, "the in-period leg still counts as one movement");

        // Panel: narrowed to s1 only, s2's interest drops out and the s2 leg stays internal.
        let only_s1 = summary(
            &schema,
            &t.user,
            &format!(r#"startDate: "2024-06-01", endDate: "2024-06-30", accountIds: ["{}"]"#, t.s1),
        )
        .await;
        assert_eq!(only_s1["accounts"].as_array().unwrap().len(), 1);
        assert_eq!((money(&only_s1, "paidIn"), money(&only_s1, "withdrawn")), (dec("300"), dec("0")));

        // Panel: a search that matches only the interest.
        let search = summary(&schema, &t.user, &format!(r#"{YEAR}, search: "interest""#)).await;
        assert_eq!((money(&search, "paidIn"), money(&search, "withdrawn")), (dec("7"), dec("0")));

        // Panel: an account selection holding no savings account has nothing to show.
        let giro = {
            let mut t2 = Tenant { user: t.user.clone(), s1: t.s1, s2: t.s2 };
            let id = checking(&db, &mut t2).await;
            (id, t2.user)
        };
        let none = summary(&schema, &giro.1, &format!(r#"{YEAR}, accountIds: ["{}"]"#, giro.0)).await;
        assert!(none.is_null());
    }

    #[tokio::test]
    async fn another_users_accounts_and_flags_never_leak_into_the_summary() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let alice = tenant(&db, &format!("sv-leak-a-{}", Uuid::new_v4().simple())).await;
        let bob = tenant(&db, &format!("sv-leak-b-{}", Uuid::new_v4().simple())).await;
        flag(&db, alice.user.user_id, alice.s1, true).await;
        common::seed_transaction(&db, alice.s1, "2024-03-01", "1000", Some("Employer")).await;
        common::seed_transaction(&db, bob.s1, "2024-03-01", "5", Some("Employer")).await;

        // Bob has marked nothing: Alice's flag is not his.
        assert!(summary(&schema, &bob.user, YEAR).await.is_null());

        // Bob can see Alice's account (shared) but flags only his own: Alice's flows stay out.
        common::link(&db, bob.user.user_id, alice.s1).await;
        let bob_user = AuthenticatedUser {
            account_ids: [bob.user.account_ids.clone(), vec![alice.s1]].concat(),
            ..bob.user.clone()
        };
        flag(&db, bob.user.user_id, bob.s1, true).await;
        let bobs = summary(&schema, &bob_user, YEAR).await;
        assert_eq!(money(&bobs, "paidIn"), dec("5"));
        assert_eq!(bobs["accounts"].as_array().unwrap().len(), 1);

        // Alice sees only hers, and asking for an account that is not hers is refused.
        let alices = summary(&schema, &alice.user, YEAR).await;
        assert_eq!(money(&alices, "paidIn"), dec("1000"));
        let query = format!(
            r#"{{ savingsSummary(filter: {{ accountIds: ["{}"] }}) {{ paidIn }} }}"#,
            bob.s1
        );
        let refused = schema.execute(authed(Request::new(query), Some(alice.user.clone()))).await;
        assert!(refused.errors[0].message.contains("not accessible"), "{:?}", refused.errors);
    }

    #[tokio::test]
    async fn a_counterpart_on_someone_elses_savings_flag_is_not_internal() {
        // Alice's s1 pairs with Bob's account; only Alice's own flags decide "internal".
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let alice = tenant(&db, &format!("sv-cp-a-{}", Uuid::new_v4().simple())).await;
        let bob = tenant(&db, &format!("sv-cp-b-{}", Uuid::new_v4().simple())).await;
        flag(&db, alice.user.user_id, alice.s1, true).await;
        flag(&db, bob.user.user_id, bob.s1, true).await;
        let out = common::seed_transaction(&db, alice.s1, "2024-03-02", "-25", Some("Bob")).await;
        let into = common::seed_transaction(&db, bob.s1, "2024-03-02", "25", Some("Alice")).await;
        pair_as_transfer(&db, out, into).await;

        let s = summary(&schema, &alice.user, YEAR).await;
        assert_eq!(money(&s, "withdrawn"), dec("25"));
        assert_eq!(s["internalTransferCount"], 0);
    }

    #[tokio::test]
    async fn savings_accounts_in_different_currencies_are_refused_not_summed() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let mut t = tenant(&db, &format!("sv-ccy-{}", Uuid::new_v4().simple())).await;
        let usd = common::seed_account(&db, "USD", "Dollar savings").await;
        common::link(&db, t.user.user_id, usd).await;
        t.user.account_ids.push(usd);
        flag(&db, t.user.user_id, t.s1, true).await;
        flag(&db, t.user.user_id, usd, true).await;

        let query = format!("{{ savingsSummary(filter: {{ {YEAR} }}) {{ paidIn }} }}");
        let response = schema.execute(authed(Request::new(query), Some(t.user.clone()))).await;
        assert!(response.errors[0].message.contains("several currencies"), "{:?}", response.errors);

        // Narrowing to one currency is the way out.
        let eur = summary(&schema, &t.user, &format!(r#"{YEAR}, accountIds: ["{}"]"#, t.s1)).await;
        assert_eq!(eur["currency"], "EUR");
    }

    async fn table_exists(conn: &DatabaseConnection) -> bool {
        let stmt = Statement::from_string(
            conn.get_database_backend(),
            "SELECT 1 AS one FROM information_schema.tables WHERE table_name = 'account_savings'",
        );
        conn.query_one(stmt).await.unwrap().is_some()
    }

    #[tokio::test]
    async fn the_migration_applies_reverses_and_reapplies() {
        use migration::{Migrator, MigratorTrait};
        let base = std::env::var("FINREPORT_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:55435/finreport_wp4".to_string());
        let admin_url = base.rsplit_once('/').unwrap().0.to_string();
        let name = format!("finreport_mig_{}", Uuid::new_v4().simple());
        let admin = sea_orm::Database::connect(format!("{admin_url}/postgres")).await.unwrap();
        admin
            .execute(Statement::from_string(admin.get_database_backend(), format!("CREATE DATABASE {name}")))
            .await
            .unwrap();

        let conn = sea_orm::Database::connect(format!("{admin_url}/{name}")).await.unwrap();
        let applied = Migrator::up(&conn, None).await;
        let table_after_up = table_exists(&conn).await;
        let reversed = Migrator::down(&conn, None).await;
        let table_after_down = table_exists(&conn).await;
        let reapplied = Migrator::up(&conn, None).await;
        let table_after_reapply = table_exists(&conn).await;
        conn.close().await.unwrap();

        admin
            .execute(Statement::from_string(
                admin.get_database_backend(),
                format!("DROP DATABASE {name} WITH (FORCE)"),
            ))
            .await
            .unwrap();

        applied.unwrap();
        reversed.unwrap();
        reapplied.unwrap();
        assert!(table_after_up, "up creates the table");
        assert!(!table_after_down, "down drops it");
        assert!(table_after_reapply, "up again re-creates it");
    }
}

/// `Transaction.label.rule` names the rule that set a label, for a whole page
/// of transactions, and degrades rather than failing when it cannot:
/// a user-labelled row has no rule, and a `rule_id` pointing at a rule that
/// has since been deleted (no foreign key guards it) yields `rule: null`
/// alongside `source: RULE`, which the UI renders as "rule deleted".
#[tokio::test]
async fn label_rule_is_exposed_for_rule_labels_only_and_survives_a_deleted_rule() {
    use chrono::Utc;
    use entity::entities::{rule, transaction_label};
    use sea_orm::{ActiveModelTrait, Set};

    let db = common::db().await;
    let (user, _) = common::seed_user(&db, "label-rule-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Label rule account").await;
    common::link(&db, user, account).await;
    let (groceries, _) = common::seed_category(&db, "groceries", "Groceries", "expense").await;

    let rule_id = uuid::Uuid::new_v4();
    rule::ActiveModel {
        id: Set(rule_id),
        name: Set("Rewe rule".to_string()),
        category_id: Set(groceries),
        conditions: Set(serde_json::json!({"counterparty_key": "rewe"})),
        priority: Set(0),
        state: Set("active".to_string()),
        origin: Set("user".to_string()),
        auto_approved: Set(false),
        user_touched: Set(true),
        confidence: Set(None),
        evidence: Set(None),
        created_at: Set(Utc::now().into()),
        revision: Set(Utc::now().into()),
    }
    .insert(db.as_ref())
    .await
    .expect("insert rule");

    let by_rule = common::seed_transaction(&db, account, "2024-09-01", "-10.00", Some("By rule")).await;
    let by_user = common::seed_transaction(&db, account, "2024-09-02", "-20.00", Some("By user")).await;
    let orphaned = common::seed_transaction(&db, account, "2024-09-03", "-30.00", Some("Orphan")).await;
    common::seed_transaction_label(&db, by_rule, Some(groceries), "rule", "resolved").await;
    common::seed_transaction_label(&db, by_user, Some(groceries), "user", "resolved").await;
    common::seed_transaction_label(&db, orphaned, Some(groceries), "rule", "resolved").await;
    for (tx, rule) in [(by_rule, rule_id), (orphaned, uuid::Uuid::new_v4())] {
        let mut label: transaction_label::ActiveModel =
            <transaction_label::Entity as sea_orm::EntityTrait>::find_by_id(tx).one(db.as_ref()).await.unwrap().unwrap().into();
        label.rule_id = Set(Some(rule));
        label.update(db.as_ref()).await.unwrap();
    }

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "label-rule-test".to_string(),
        account_ids: vec![account],
    };
    let query = r#"{ transactions(filter: { startDate: "2024-09-01", endDate: "2024-09-03" }) {
        items { id label { source rule { id name state } } } } }"#;
    let response = schema.execute(authed(Request::new(query), Some(caller))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let label_of = |id: uuid::Uuid| {
        data["transactions"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == id.to_string())
            .unwrap()["label"]
            .clone()
    };

    let ruled = label_of(by_rule);
    assert_eq!(ruled["source"], "RULE");
    assert_eq!(ruled["rule"]["id"], rule_id.to_string());
    assert_eq!(ruled["rule"]["name"], "Rewe rule");
    assert_eq!(ruled["rule"]["state"], "ACTIVE");

    let user_labelled = label_of(by_user);
    assert_eq!(user_labelled["source"], "USER");
    assert!(user_labelled["rule"].is_null());

    let gone = label_of(orphaned);
    assert_eq!(gone["source"], "RULE", "the label still says a rule set it");
    assert!(gone["rule"].is_null(), "a deleted rule is null, not an error");
}

// ---------------------------------------------------------------------------
// Amazon purchase orders: the upload mutation and the match report
// ---------------------------------------------------------------------------

mod purchase_orders {
    use super::*;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
    use serde_json::Value;
    use uuid::Uuid;
    use webapp::kafka::purchase_orders::TOPIC_PURCHASE_ORDER;

    const HEADER: &str = "Order ID,Order Date,Total Amount,Total Savings,Status,Item ASIN,Item Quantity,Item Price,Item Discount,Promotions,Item Title,Item URL,Details URL,Recipient Name,Recipient Street,Recipient City,Recipient State,Recipient Zip,Recipient Country";

    /// An id in Amazon's shape, unique per call so tests sharing a database
    /// never meet each other's orders.
    fn order_id() -> String {
        let n = Uuid::new_v4().as_u128();
        format!("{:03}-{:07}-{:07}", n % 1000, (n >> 16) % 10_000_000, (n >> 48) % 10_000_000)
    }

    /// One single-item order per row: (order id, total, item price, status).
    fn csv(rows: &[(&str, &str, &str, &str)]) -> String {
        let mut out = format!("{HEADER}\n");
        for (i, (id, total, price, status)) in rows.iter().enumerate() {
            out.push_str(&format!(
                "{id},2026-10-01,{total} EUR,,{status},B00{i},1,{price},,,Item {i},u,d,Home Person,Home Street 1,Hometown,,12345,DE\n"
            ));
        }
        out
    }

    async fn kafka_with_topic() -> (testcontainers::ContainerAsync<testcontainers_modules::kafka::apache::Kafka>, String) {
        use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
        use rdkafka::client::DefaultClientContext;
        use rdkafka::ClientConfig;
        use testcontainers::runners::AsyncRunner;
        use testcontainers_modules::kafka::apache::{Kafka, KAFKA_PORT};

        let container = Kafka::default().start().await.expect("start Kafka testcontainer");
        let host = container.get_host().await.unwrap();
        let port = container.get_host_port_ipv4(KAFKA_PORT).await.unwrap();
        let bootstrap = format!("{host}:{port}");
        let admin: AdminClient<DefaultClientContext> =
            ClientConfig::new().set("bootstrap.servers", &bootstrap).create().unwrap();
        admin
            .create_topics(
                &[
                    NewTopic::new(TOPIC_PURCHASE_ORDER, 1, TopicReplication::Fixed(1)),
                    NewTopic::new(TOPIC_USER_LABEL, 1, TopicReplication::Fixed(1)),
                    NewTopic::new(webapp::kafka::labeling::TOPIC_LLM_CACHE, 1, TopicReplication::Fixed(1)),
                ],
                &AdminOptions::new().request_timeout(Some(Duration::from_secs(10))),
            )
            .await
            .expect("create purchase-order topic");
        (container, bootstrap)
    }

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

    async fn bank_line(db: &DatabaseConnection, account: Uuid, date: &str, amount: &str, name: &str, description: &str) -> Uuid {
        let id = common::seed_transaction(db, account, date, amount, Some(name)).await;
        let mut model: entity::entities::transaction::ActiveModel =
            entity::entities::transaction::Entity::find_by_id(id).one(db).await.unwrap().unwrap().into();
        model.description = Set(Some(description.to_string()));
        model.update(db).await.unwrap();
        id
    }

    async fn gql(schema: &webapp::graphql::AppSchema, user: &AuthenticatedUser, query: String) -> async_graphql::Response {
        schema.execute(authed(Request::new(query), Some(user.clone()))).await
    }

    fn upload_query(files: &[String]) -> String {
        let list = files.iter().map(|f| serde_json::to_string(f).unwrap()).collect::<Vec<_>>().join(",");
        format!(
            "mutation {{ uploadPurchaseOrders(files: [{list}]) {{ newOrders updatedOrders unchangedOrders files {{ index orderCount error }} }} }}"
        )
    }

    async fn stored_count(db: &DatabaseConnection, user: Uuid) -> usize {
        entity::entities::purchase_order::Entity::find()
            .all(db)
            .await
            .unwrap()
            .into_iter()
            .filter(|o| o.user_id == user)
            .count()
    }

    const REPORT: &str = "{ purchaseOrderMatches { orderCount \
        matches { order { orderId } kind state reasons { code message } charges { transactionId amount } } \
        unmatchedOrders { orderId } unmatchedCharges { transactionId } subscriptionCharges { transactionId } } }";

    #[tokio::test]
    async fn upload_stores_orders_and_a_re_upload_changes_nothing() {
        let db = common::db().await;
        let (_kafka, bootstrap) = kafka_with_topic().await;
        let schema = create_schema(db.clone(), settings_with_kafka(&bootstrap));
        let t = tenant(&db, &format!("po-up-{}", Uuid::new_v4().simple())).await;
        let (a, b) = (order_id(), order_id());
        let file = csv(&[(&a, "10.00", "10.00", "Delivered"), (&b, "5.00", "5.00", "Delivered")]);

        let first = gql(&schema, &t.user, upload_query(&[file.clone()])).await;
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        let first = first.data.into_json().unwrap()["uploadPurchaseOrders"].clone();
        assert_eq!((first["newOrders"].as_i64(), first["unchangedOrders"].as_i64()), (Some(2), Some(0)));
        assert_eq!(stored_count(&db, t.user.user_id).await, 2);

        let again = gql(&schema, &t.user, upload_query(&[file])).await;
        assert!(again.errors.is_empty(), "{:?}", again.errors);
        let again = again.data.into_json().unwrap()["uploadPurchaseOrders"].clone();
        assert_eq!(
            (again["newOrders"].as_i64(), again["updatedOrders"].as_i64(), again["unchangedOrders"].as_i64()),
            (Some(0), Some(0), Some(2)),
            "nothing is republished for an unchanged order"
        );
        assert_eq!(stored_count(&db, t.user.user_id).await, 2);

        let moved = gql(&schema, &t.user, upload_query(&[csv(&[(&a, "10.00", "10.00", "Return started")])])).await;
        let moved = moved.data.into_json().unwrap()["uploadPurchaseOrders"].clone();
        assert_eq!((moved["updatedOrders"].as_i64(), moved["newOrders"].as_i64()), (Some(1), Some(0)));
    }

    async fn llm_cache_rows(db: &DatabaseConnection) -> u64 {
        use sea_orm::PaginatorTrait;
        entity::entities::llm_label_cache::Entity::find().count(db).await.unwrap()
    }

    /// Cache rows are global, so the count is compared across the upload while
    /// no other test in this binary writes to it (they all use the `fake`
    /// provider, which the upload never warms against).
    #[tokio::test]
    async fn the_fake_provider_writes_no_cache_rows_on_upload_and_a_re_upload_warms_nothing() {
        let db = common::db().await;
        let (_kafka, bootstrap) = kafka_with_topic().await;
        let mut settings = (*settings_with_kafka(&bootstrap)).clone();
        assert_eq!(settings.llm_provider, "fake", "the default");
        settings.llm_max_requests_per_run = 1_000;
        let schema = create_schema(db.clone(), Arc::new(settings));
        let t = tenant(&db, &format!("po-warm-{}", Uuid::new_v4().simple())).await;
        // Titles unique to this run: a cache row left by an earlier run would
        // otherwise answer from the cache and hide a write.
        let tag = Uuid::new_v4().simple().to_string();
        let file = csv(&[(&order_id(), "10.00", "10.00", "Delivered"), (&order_id(), "5.00", "5.00", "Delivered")])
            .replace("Item 0", &format!("Zw{tag} Item 0"))
            .replace("Item 1", &format!("Zw{tag} Item 1"));

        let before = llm_cache_rows(&db).await;
        let first = gql(&schema, &t.user, upload_query(&[file.clone()])).await;
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        let first = first.data.into_json().unwrap()["uploadPurchaseOrders"].clone();
        assert_eq!(first["newOrders"], 2, "the orders are still imported");
        assert_eq!(llm_cache_rows(&db).await, before, "fake answers are never stored on upload");

        let again = gql(&schema, &t.user, upload_query(&[file])).await;
        assert!(again.errors.is_empty(), "{:?}", again.errors);
        let again = again.data.into_json().unwrap()["uploadPurchaseOrders"].clone();
        assert_eq!((again["newOrders"].as_i64(), again["unchangedOrders"].as_i64()), (Some(0), Some(2)));
        assert_eq!(llm_cache_rows(&db).await, before, "an unchanged order warms nothing");
    }

    #[tokio::test]
    async fn a_bad_file_is_rejected_whole_while_a_good_one_imports_and_no_cell_is_echoed() {
        let db = common::db().await;
        let (_kafka, bootstrap) = kafka_with_topic().await;
        let schema = create_schema(db.clone(), settings_with_kafka(&bootstrap));
        let t = tenant(&db, &format!("po-bad-{}", Uuid::new_v4().simple())).await;
        let id = order_id();
        let good = csv(&[(&id, "10.00", "10.00", "Delivered")]);
        let bad = good.replace("2026-10-01", "not-a-date");
        assert_ne!(bad, good);

        let r = gql(&schema, &t.user, upload_query(&[bad, good])).await;
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let data = r.data.into_json().unwrap()["uploadPurchaseOrders"].clone();
        assert_eq!(data["newOrders"], 1);
        let error = data["files"][0]["error"].as_str().expect("first file rejected").to_string();
        assert!(data["files"][1]["error"].is_null());
        for leak in ["Home Person", "Home Street", "Hometown", "12345", "not-a-date"] {
            assert!(!error.contains(leak), "error echoed a cell: {error}");
        }
        assert_eq!(stored_count(&db, t.user.user_id).await, 1);

        // A wrong header imports nothing and says so without quoting a row.
        let r = gql(&schema, &t.user, upload_query(&["Foo,Bar\nHome Person,Home Street 1\n".to_string()])).await;
        let data = r.data.into_json().unwrap()["uploadPurchaseOrders"].clone();
        assert_eq!(data["newOrders"], 0);
        assert!(!data["files"][0]["error"].as_str().unwrap().contains("Home"));
        assert_eq!(stored_count(&db, t.user.user_id).await, 1);
    }

    #[tokio::test]
    async fn upload_without_a_broker_fails_with_kafka_unavailable() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let t = tenant(&db, &format!("po-nok-{}", Uuid::new_v4().simple())).await;
        let r = gql(&schema, &t.user, upload_query(&[csv(&[(&order_id(), "1.00", "1.00", "Delivered")])])).await;
        let code = r.errors[0].extensions.as_ref().and_then(|e| e.get("code")).cloned();
        assert_eq!(code, Some(async_graphql::Value::from("KAFKA_UNAVAILABLE")));
        assert_eq!(stored_count(&db, t.user.user_id).await, 0);
    }

    #[tokio::test]
    async fn the_report_tells_apply_review_subscription_and_unclaimed_apart_and_never_crosses_users() {
        let db = common::db().await;
        let (_kafka, bootstrap) = kafka_with_topic().await;
        let schema = create_schema(db.clone(), settings_with_kafka(&bootstrap));
        let alice = tenant(&db, &format!("po-rep-a-{}", Uuid::new_v4().simple())).await;
        let bob = tenant(&db, &format!("po-rep-b-{}", Uuid::new_v4().simple())).await;
        let (clean, split, gift) = (order_id(), order_id(), order_id());

        // clean: one order, one charge. split: one item, two charges (cannot
        // be assigned). gift: no bank line at all.
        let file = csv(&[
            (&clean, "13.45", "13.45", "Delivered"),
            (&split, "22.59", "22.59", "Delivered"),
            (&gift, "5.00", "5.00", "Delivered"),
        ]);
        let up = gql(&schema, &alice.user, upload_query(&[file])).await;
        assert!(up.errors.is_empty(), "{:?}", up.errors);

        let amazon = "AMAZON PAYMENTS EUROPE S.C.A.";
        let line = |id: &str| format!("01{id} AMZN Mktp DE");
        let clean_tx = bank_line(&db, alice.account, "2026-10-08", "-13.45", amazon, &line(&clean)).await;
        let s1 = bank_line(&db, alice.account, "2026-09-24", "-13.60", amazon, &line(&split)).await;
        let s2 = bank_line(&db, alice.account, "2026-09-23", "-8.99", amazon, &line(&split)).await;
        let sub = bank_line(&db, alice.account, "2026-10-07", "-8.99", "AMAZON EU S.A R.L.", "01D01-8109513-9170236 AMZNPrime DE").await;
        let stray = bank_line(&db, alice.account, "2026-10-02", "-7.77", amazon, &line("999-9999999-9999999")).await;
        // Bob has an Amazon line carrying Alice's clean order id: it must not
        // show on Alice's report, and Bob (no orders) sees an empty one.
        let bobs = bank_line(&db, bob.account, "2026-10-08", "-13.45", amazon, &line(&clean)).await;

        let r = gql(&schema, &alice.user, REPORT.to_string()).await;
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let report = r.data.into_json().unwrap()["purchaseOrderMatches"].clone();
        assert_eq!(report["orderCount"], 3);
        let matches = report["matches"].as_array().unwrap();
        let find = |id: &str| matches.iter().find(|m| m["order"]["orderId"] == id).unwrap_or_else(|| panic!("no match for {id}"));

        let c = find(&clean);
        assert_eq!(c["state"], "APPLY");
        assert!(c["reasons"].as_array().unwrap().is_empty());
        assert_eq!(c["charges"][0]["transactionId"], clean_tx.to_string());

        let s = find(&split);
        assert_eq!(s["state"], "NEEDS_REVIEW");
        assert_eq!(s["reasons"][0]["code"], "ITEMS_DO_NOT_RECONCILE_TO_CHARGES");
        assert_eq!(s["reasons"][0]["message"], "order spans 2 charges; items do not reconcile to them");
        let charged: Vec<String> = s["charges"].as_array().unwrap().iter().map(|c| c["transactionId"].as_str().unwrap().to_string()).collect();
        assert!(charged.contains(&s1.to_string()) && charged.contains(&s2.to_string()));

        assert_eq!(report["unmatchedOrders"][0]["orderId"], gift);
        assert_eq!(report["subscriptionCharges"][0]["transactionId"], sub.to_string());
        assert_eq!(report["unmatchedCharges"][0]["transactionId"], stray.to_string());
        assert!(!report.to_string().contains(&bobs.to_string()), "another user's transaction leaked");

        let theirs = gql(&schema, &bob.user, REPORT.to_string()).await;
        let theirs: Value = theirs.data.into_json().unwrap()["purchaseOrderMatches"].clone();
        assert_eq!(theirs["orderCount"], 0, "Alice's orders are not Bob's");
        assert!(theirs["matches"].as_array().unwrap().is_empty());
    }

    // -----------------------------------------------------------------
    // Applying a match: the first thing here that changes the user's data
    // -----------------------------------------------------------------

    /// One two-item order whose items (13.60 + 8.99) equal the 22.59 charged.
    fn two_item_csv(id: &str) -> String {
        format!(
            "{HEADER}\n\
             {id},2026-10-01,22.59 EUR,,Delivered,B001,1,13.60,,,Anker Ladegeraet,u,d,Home Person,Home Street 1,Hometown,,12345,DE\n\
             {id},2026-10-01,22.59 EUR,,Delivered,B002,1,8.99,,,USB Kabel,u,d,Home Person,Home Street 1,Hometown,,12345,DE\n"
        )
    }

    const AMAZON: &str = "AMAZON PAYMENTS EUROPE S.C.A.";

    struct Cats {
        a: String,
        b: String,
        rest: String,
    }

    async fn cats(db: &DatabaseConnection) -> Cats {
        let n = Uuid::new_v4().simple().to_string();
        let (_, a) = common::seed_category(db, &format!("expense.poa{n}"), "PO A", "EXPENSE").await;
        let (_, b) = common::seed_category(db, &format!("expense.pob{n}"), "PO B", "EXPENSE").await;
        let (_, rest) = common::seed_category(db, &format!("expense.por{n}"), "PO Rest", "EXPENSE").await;
        Cats { a, b, rest }
    }

    fn apply_query(order: &str, c: &Cats) -> String {
        format!(
            r#"mutation {{ applyPurchaseOrderSplit(input: {{ orderId: "{order}", itemCategories: ["{}", "{}"], remainderCategory: "{}" }}) {{
                orderId outcomes {{ transactionId status reason parts {{ amount categorySlug label }} }} }} }}"#,
            c.a, c.b, c.rest
        )
    }

    fn undo_query(order: &str) -> String {
        format!(
            r#"mutation {{ undoPurchaseOrderSplit(orderId: "{order}") {{ orderId outcomes {{ transactionId status reason }} }} }}"#
        )
    }

    async fn splits_of(db: &DatabaseConnection, tx: Uuid) -> Vec<(i32, Decimal)> {
        use sea_orm::{ColumnTrait, QueryFilter};
        let mut v: Vec<_> = entity::entities::transaction_split::Entity::find()
            .filter(entity::entities::transaction_split::Column::TransactionId.eq(tx))
            .all(db)
            .await
            .unwrap()
            .into_iter()
            .map(|s| (s.part_index, s.amount))
            .collect();
        v.sort();
        v
    }

    async fn applications_of(db: &DatabaseConnection, tx: Uuid) -> usize {
        entity::entities::purchase_order_split::Entity::find_by_id(tx).one(db).await.unwrap().iter().count()
    }

    struct Applied {
        schema: webapp::graphql::AppSchema,
        _kafka: testcontainers::ContainerAsync<testcontainers_modules::kafka::apache::Kafka>,
        db: Arc<DatabaseConnection>,
        t: Tenant,
        order: String,
        tx: Uuid,
        cats: Cats,
    }

    /// An uploaded two-item order with its single matching bank line.
    async fn ready() -> Applied {
        let db = common::db().await;
        let (kafka, bootstrap) = kafka_with_topic().await;
        let schema = create_schema(db.clone(), settings_with_kafka(&bootstrap));
        let t = tenant(&db, &format!("po-apply-{}", Uuid::new_v4().simple())).await;
        let order = order_id();
        let up = gql(&schema, &t.user, upload_query(&[two_item_csv(&order)])).await;
        assert!(up.errors.is_empty(), "{:?}", up.errors);
        let tx = bank_line(&db, t.account, "2026-10-02", "-22.59", AMAZON, &format!("01{order} AMZN Mktp DE")).await;
        let cats = cats(&db).await;
        Applied { schema, _kafka: kafka, db, t, order, tx, cats }
    }

    impl Applied {
        async fn run(&self, query: String) -> async_graphql::Response {
            gql(&self.schema, &self.t.user, query).await
        }
        async fn apply(&self) -> Value {
            let r = self.run(apply_query(&self.order, &self.cats)).await;
            assert!(r.errors.is_empty(), "{:?}", r.errors);
            r.data.into_json().unwrap()["applyPurchaseOrderSplit"]["outcomes"][0].clone()
        }
    }

    const REPORT_WITH_APPLICATIONS: &str =
        "{ purchaseOrderMatches { matches { state applications { transactionId intact parts { amount label } } } } }";

    #[tokio::test]
    async fn applying_splits_the_charge_into_parts_that_sum_to_it_exactly() {
        let w = ready().await;
        let out = w.apply().await;
        assert_eq!(out["status"], "APPLIED", "{out}");
        let rows = splits_of(&w.db, w.tx).await;
        assert_eq!(rows.len(), 2, "one part per item; nothing is left over to need a remainder part");
        assert_eq!(rows.iter().map(|r| r.1).sum::<Decimal>(), Decimal::from_str("-22.59").unwrap());
        assert_eq!(rows[0].1, Decimal::from_str("-13.60").unwrap());
        assert_eq!(out["parts"][0]["label"], "Anker Ladegeraet");
        assert_eq!(out["parts"][1]["label"], "USB Kabel");

        let r = w.run(REPORT_WITH_APPLICATIONS.to_string()).await;
        let report = r.data.into_json().unwrap()["purchaseOrderMatches"].clone();
        let app = &report["matches"][0]["applications"][0];
        assert_eq!(app["intact"], true);
        assert_eq!(app["parts"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn applying_twice_changes_nothing() {
        let w = ready().await;
        assert_eq!(w.apply().await["status"], "APPLIED");
        let before = splits_of(&w.db, w.tx).await;
        let again = w.apply().await;
        assert_eq!(again["status"], "ALREADY_APPLIED", "{again}");
        assert_eq!(splits_of(&w.db, w.tx).await, before);
        // A re-upload of the same file followed by another apply is also a no-op.
        let up = w.run(upload_query(&[two_item_csv(&w.order)])).await;
        assert!(up.errors.is_empty());
        assert_eq!(w.apply().await["status"], "ALREADY_APPLIED");
        assert_eq!(splits_of(&w.db, w.tx).await, before);
        assert_eq!(applications_of(&w.db, w.tx).await, 1);
        // Different categories on a second apply are refused, not stacked.
        let other = Cats { a: w.cats.b.clone(), b: w.cats.a.clone(), rest: w.cats.rest.clone() };
        let r = w.run(apply_query(&w.order, &other)).await;
        let out = r.data.into_json().unwrap()["applyPurchaseOrderSplit"]["outcomes"][0].clone();
        assert_eq!(out["status"], "SKIPPED");
        assert_eq!(splits_of(&w.db, w.tx).await, before);
    }

    #[tokio::test]
    async fn a_users_own_split_is_skipped_and_never_overwritten() {
        let w = ready().await;
        let manual = format!(
            r#"mutation {{ splitTransaction(transactionId: "{}", parts: [
                {{ amount: "-20.00", categorySlug: "{}" }}, {{ amount: "-2.59", categorySlug: "{}" }}
            ]) {{ id }} }}"#,
            w.tx, w.cats.a, w.cats.b
        );
        let r = w.run(manual).await;
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let before = splits_of(&w.db, w.tx).await;
        let out = w.apply().await;
        assert_eq!(out["status"], "SKIPPED", "{out}");
        assert!(out["reason"].as_str().unwrap().contains("split"));
        assert_eq!(splits_of(&w.db, w.tx).await, before, "the user's split is untouched");
        assert_eq!(applications_of(&w.db, w.tx).await, 0);

        // Undo has nothing of ours to remove, so the user's split survives it too.
        let undo = w.run(undo_query(&w.order)).await;
        assert!(undo.errors.is_empty());
        assert_eq!(splits_of(&w.db, w.tx).await, before);
    }

    #[tokio::test]
    async fn a_pinned_category_is_skipped_and_stays_pinned() {
        let w = ready().await;
        let pin = format!(
            r#"mutation {{ setTransactionCategory(transactionId: "{}", categorySlug: "{}") {{ id }} }}"#,
            w.tx, w.cats.a
        );
        assert!(w.run(pin).await.errors.is_empty());
        let out = w.apply().await;
        assert_eq!(out["status"], "SKIPPED", "{out}");
        assert!(splits_of(&w.db, w.tx).await.is_empty());
        let pinned = entity::entities::transaction_user_label::Entity::find_by_id(w.tx)
            .one(&*w.db)
            .await
            .unwrap()
            .unwrap();
        assert!(pinned.category_id.is_some(), "the user's category survived");
    }

    #[tokio::test]
    async fn a_needs_review_match_cannot_be_applied() {
        let db = common::db().await;
        let (_kafka, bootstrap) = kafka_with_topic().await;
        let schema = create_schema(db.clone(), settings_with_kafka(&bootstrap));
        let t = tenant(&db, &format!("po-nr-{}", Uuid::new_v4().simple())).await;
        let id = order_id();
        let up = gql(&schema, &t.user, upload_query(&[csv(&[(&id, "22.59", "22.59", "Delivered")])])).await;
        assert!(up.errors.is_empty());
        let s1 = bank_line(&db, t.account, "2026-09-24", "-13.60", AMAZON, &format!("01{id} AMZN Mktp DE")).await;
        let s2 = bank_line(&db, t.account, "2026-09-23", "-8.99", AMAZON, &format!("01{id} AMZN Mktp DE")).await;
        let c = cats(&db).await;
        let r = gql(&schema, &t.user, apply_query(&id, &c)).await;
        assert!(!r.errors.is_empty(), "a NeedsReview order must be refused");
        assert!(r.errors[0].message.contains("needs review"), "{:?}", r.errors);
        assert!(splits_of(&db, s1).await.is_empty() && splits_of(&db, s2).await.is_empty());
        // An id nobody uploaded is refused the same way.
        let r = gql(&schema, &t.user, apply_query("000-0000000-0000000", &c)).await;
        assert!(!r.errors.is_empty());
    }

    #[tokio::test]
    async fn undo_restores_the_state_before_applying_and_only_that() {
        let w = ready().await;
        // Prior state: a tag the user set, no split, no category.
        let tag = format!(
            r#"mutation {{ setTransactionTags(transactionId: "{}", tags: ["amazon-test"]) {{ id }} }}"#,
            w.tx
        );
        let tagged = w.run(tag).await;
        assert!(tagged.errors.is_empty(), "{:?}", tagged.errors);
        let tags_of = |label: Option<entity::entities::transaction_user_label::Model>| label.map(|l| (l.category_id, l.note));
        let label_before = entity::entities::transaction_user_label::Entity::find_by_id(w.tx).one(&*w.db).await.unwrap();

        assert_eq!(w.apply().await["status"], "APPLIED");
        let undo = w.run(undo_query(&w.order)).await;
        assert!(undo.errors.is_empty(), "{:?}", undo.errors);
        let out = undo.data.into_json().unwrap()["undoPurchaseOrderSplit"]["outcomes"][0].clone();
        assert_eq!(out["status"], "UNDONE", "{out}");
        assert!(splits_of(&w.db, w.tx).await.is_empty());
        assert_eq!(applications_of(&w.db, w.tx).await, 0);
        let label_after = entity::entities::transaction_user_label::Entity::find_by_id(w.tx).one(&*w.db).await.unwrap();
        assert_eq!(tags_of(label_after), tags_of(label_before), "category and note are as before applying");

        // Undoing again is a no-op, and the order can be applied afresh.
        let again = w.run(undo_query(&w.order)).await;
        assert!(again.data.into_json().unwrap()["undoPurchaseOrderSplit"]["outcomes"].as_array().unwrap().is_empty());
        assert_eq!(w.apply().await["status"], "APPLIED");
    }

    #[tokio::test]
    async fn undo_leaves_a_split_the_user_changed_after_applying() {
        let w = ready().await;
        assert_eq!(w.apply().await["status"], "APPLIED");
        let mine = format!(
            r#"mutation {{ splitTransaction(transactionId: "{}", parts: [
                {{ amount: "-20.00", categorySlug: "{}" }}, {{ amount: "-2.59", categorySlug: "{}" }}
            ]) {{ id }} }}"#,
            w.tx, w.cats.a, w.cats.b
        );
        assert!(w.run(mine).await.errors.is_empty());
        let before = splits_of(&w.db, w.tx).await;
        let undo = w.run(undo_query(&w.order)).await;
        let out = undo.data.into_json().unwrap()["undoPurchaseOrderSplit"]["outcomes"][0].clone();
        assert_eq!(out["status"], "LEFT_ALONE", "{out}");
        assert_eq!(splits_of(&w.db, w.tx).await, before, "the user's later split survives undo");
        assert_eq!(applications_of(&w.db, w.tx).await, 0, "the stale record is forgotten");
    }

    #[tokio::test]
    async fn another_users_transaction_cannot_be_split_or_unsplit() {
        let w = ready().await;
        let bob = tenant(&w.db, &format!("po-apply-bob-{}", Uuid::new_v4().simple())).await;
        // Bob uploads the very same order, but his accounts hold no matching line.
        let up = gql(&w.schema, &bob.user, upload_query(&[two_item_csv(&w.order)])).await;
        assert!(up.errors.is_empty(), "{:?}", up.errors);
        let r = gql(&w.schema, &bob.user, apply_query(&w.order, &w.cats)).await;
        assert!(!r.errors.is_empty(), "Bob has no charge for this order");
        assert!(splits_of(&w.db, w.tx).await.is_empty(), "Alice's transaction was not touched");

        assert_eq!(w.apply().await["status"], "APPLIED");
        let before = splits_of(&w.db, w.tx).await;
        let undo = gql(&w.schema, &bob.user, undo_query(&w.order)).await;
        assert!(undo.data.into_json().unwrap()["undoPurchaseOrderSplit"]["outcomes"].as_array().unwrap().is_empty());
        assert_eq!(splits_of(&w.db, w.tx).await, before, "Bob cannot undo Alice's split");
    }
}

// ---------------------------------------------------------------------------
// Scope-aware transfer exclusion: a flagged transfer leg is dropped only when
// its counterpart is inside the *effective* scope (the selected
// `accountIds`, else the whole authorisation scope). Excluding transfers is
// right at household level — the money did not leave — and wrong per account,
// where an inbound transfer *is* that account's income. Before this, the
// detector flagged both legs and both were dropped, so the receiving account
// showed only its outflows and read as a phantom deficit.
//
// Test isolation: the integration database is shared and sibling tests in the
// same binary run concurrently, so every test here seeds its own tenant and
// asserts only on that tenant's accounts. `cashflowSummary`/`cashflowGraph`
// are already scoped to the caller, so these assertions cannot be perturbed
// by another test's rows.
// ---------------------------------------------------------------------------

mod transfer_scope {
    use super::*;
    use chrono::Utc;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
    use uuid::Uuid;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    /// A user with `labels.len()` linked accounts, named uniquely per run.
    async fn tenant(
        db: &DatabaseConnection,
        name: &str,
        labels: &[&str],
    ) -> (AuthenticatedUser, Vec<Uuid>) {
        let (user_id, username) = common::seed_user(db, name, "pw").await;
        let mut ids = Vec::new();
        for label in labels {
            let id = common::seed_account(db, "EUR", label).await;
            common::link(db, user_id, id).await;
            ids.push(id);
        }
        (
            AuthenticatedUser { user_id, username, account_ids: ids.clone() },
            ids,
        )
    }

    fn insight(id: Uuid, counterpart: Option<Uuid>) -> entity::entities::transaction_insight::ActiveModel {
        entity::entities::transaction_insight::ActiveModel {
            transaction_id: Set(id),
            is_transfer: Set(true),
            transfer_counterpart_id: Set(counterpart),
            transfer_match: Set(Some("amount_date".to_string())),
            is_recurring: Set(false),
            recurring_series_id: Set(None),
            recurring_cadence: Set(None),
            recurring_median_amount: Set(None),
            detected_at: Set(Utc::now().into()),
            revision: Set(Utc::now().into()),
        }
    }

    /// Marks two transactions as the two legs of one detected transfer —
    /// exactly what `detect::transfer` does: it flags **both** sides.
    async fn pair_as_transfer(db: &DatabaseConnection, a: Uuid, b: Uuid) {
        for (id, other) in [(a, b), (b, a)] {
            insight(id, Some(other)).insert(db).await.unwrap();
        }
    }

    /// A leg flagged as a transfer whose counterpart was never resolved.
    async fn flag_without_counterpart(db: &DatabaseConnection, id: Uuid) {
        insight(id, None).insert(db).await.unwrap();
    }

    const MAY: &str = r#"startDate: "2024-05-01", endDate: "2024-05-31""#;

    fn accounts_arg(ids: &[Uuid]) -> String {
        let list: Vec<String> = ids.iter().map(|id| format!("\"{id}\"")).collect();
        format!("accountIds: [{}]", list.join(", "))
    }

    /// `(income, spending, net, transactionCount)` from `cashflowSummary`.
    async fn summary(
        schema: &webapp::graphql::AppSchema,
        user: &AuthenticatedUser,
        filter: &str,
    ) -> (Decimal, Decimal, Decimal, i64) {
        let query = format!(
            "{{ cashflowSummary(filter: {{ {filter} }}, granularity: MONTH) \
               {{ total {{ income spending net transactionCount }} }} }}"
        );
        let response = schema
            .execute(authed(Request::new(query), Some(user.clone())))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let total = response.data.into_json().unwrap()["cashflowSummary"]["total"].clone();
        (
            dec(total["income"].as_str().unwrap()),
            dec(total["spending"].as_str().unwrap()),
            dec(total["net"].as_str().unwrap()),
            total["transactionCount"].as_i64().unwrap(),
        )
    }

    /// `(income into accounts, spending out of accounts)` as the Sankey
    /// renders it — the same reconciliation
    /// `cashflow_graph_conserves_flow_and_reconciles_with_summary` uses, so
    /// the graph and the summary can be checked against each other.
    async fn graph_totals(
        schema: &webapp::graphql::AppSchema,
        user: &AuthenticatedUser,
        filter: &str,
        grouping: Option<&str>,
    ) -> (Decimal, Decimal, serde_json::Value) {
        let grouping = grouping
            .map(|g| format!(", grouping: {{ dimensions: [{g}] }}"))
            .unwrap_or_default();
        let query = format!(
            "{{ cashflowGraph(filter: {{ {filter} }}{grouping}) \
               {{ nodes {{ id kind value refType refId }} links {{ sourceId targetId value }} }} }}"
        );
        let response = schema
            .execute(authed(Request::new(query), Some(user.clone())))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap()["cashflowGraph"].clone();
        let nodes = data["nodes"].as_array().unwrap().clone();
        let links = data["links"].as_array().unwrap().clone();
        let kind = |id: &serde_json::Value| -> String {
            nodes
                .iter()
                .find(|n| &n["id"] == id)
                .map(|n| n["kind"].as_str().unwrap().to_string())
                .unwrap_or_default()
        };
        let value = |l: &serde_json::Value| dec(l["value"].as_str().unwrap());
        let income: Decimal = links
            .iter()
            .filter(|l| kind(&l["targetId"]) == "ACCOUNT" && kind(&l["sourceId"]) != "DEFICIT")
            .map(value)
            .sum();
        let spending: Decimal = links
            .iter()
            .filter(|l| kind(&l["sourceId"]) == "ACCOUNT" && kind(&l["targetId"]) != "NET")
            .map(value)
            .sum();
        (income, spending, data)
    }

    /// Salary into `a`, a slice of it transferred to `b` (both legs flagged
    /// and paired, as the detector leaves them), plus ordinary spending on
    /// `b` — the exact shape the user described.
    async fn salary_and_bucket(db: &DatabaseConnection, a: Uuid, b: Uuid) {
        common::seed_transaction(db, a, "2024-05-01", "3000.00", Some("Employer")).await;
        let out = common::seed_transaction(db, a, "2024-05-02", "-1000.00", Some("Own account")).await;
        let inn = common::seed_transaction(db, b, "2024-05-02", "1000.00", Some("Own account")).await;
        pair_as_transfer(db, out, inn).await;
        common::seed_transaction(db, b, "2024-05-10", "-400.00", Some("Supermarkt")).await;
    }

    /// (1) The whole-household view is unchanged by construction: both legs
    /// are inside the effective scope, so both are still dropped by
    /// `cashflowSummary` *and* `cashflowGraph`. This is the reason this
    /// approach was chosen over a new filter mode, so it is a test rather
    /// than an assumption.
    #[tokio::test]
    async fn the_whole_household_view_still_excludes_both_legs() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let (user, ids) = tenant(&db, "ts-household", &["Salary", "Bucket"]).await;
        salary_and_bucket(&db, ids[0], ids[1]).await;

        // Salary 3000 in, 400 spent. The 1000 moved between the two accounts
        // appears nowhere: the money did not leave the household.
        assert_eq!(
            summary(&schema, &user, MAY).await,
            (dec("3000.0000"), dec("400.0000"), dec("2600.0000"), 2)
        );

        let (g_income, g_spending, _) = graph_totals(&schema, &user, MAY, None).await;
        assert_eq!((g_income, g_spending), (dec("3000.0000"), dec("400.0000")));

        // An explicit, redundant selection of *all* accounts is the same view.
        let all = format!("{MAY}, {}", accounts_arg(&ids));
        assert_eq!(
            summary(&schema, &user, &all).await,
            (dec("3000.0000"), dec("400.0000"), dec("2600.0000"), 2)
        );
    }

    /// (2) The phantom deficit: with only the receiving account selected, the
    /// inbound leg's counterpart is outside the scope, so it counts as that
    /// account's income and the account is no longer in deficit.
    #[tokio::test]
    async fn the_receiving_account_alone_counts_the_inbound_leg_as_income() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let (user, ids) = tenant(&db, "ts-receiving", &["Salary", "Bucket"]).await;
        salary_and_bucket(&db, ids[0], ids[1]).await;

        let only_bucket = format!("{MAY}, {}", accounts_arg(&ids[1..2]));
        let (income, spending, net, count) = summary(&schema, &user, &only_bucket).await;
        assert_eq!(income, dec("1000.0000"), "the transfer in is this account's income");
        assert_eq!(spending, dec("400.0000"));
        assert_eq!(net, dec("600.0000"));
        assert!(net > Decimal::ZERO, "the account must not read as a deficit");
        assert_eq!(count, 2);

        let (g_income, g_spending, _) = graph_totals(&schema, &user, &only_bucket, None).await;
        assert_eq!(
            (g_income, g_spending),
            (dec("1000.0000"), dec("400.0000")),
            "cashflowGraph must not disagree with cashflowSummary"
        );
    }

    /// (3) Symmetry: the sending account alone counts the outbound leg as
    /// spending — the allocation left *that* account.
    #[tokio::test]
    async fn the_sending_account_alone_counts_the_outbound_leg_as_spending() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let (user, ids) = tenant(&db, "ts-sending", &["Salary", "Bucket"]).await;
        salary_and_bucket(&db, ids[0], ids[1]).await;

        let only_salary = format!("{MAY}, {}", accounts_arg(&ids[0..1]));
        assert_eq!(
            summary(&schema, &user, &only_salary).await,
            (dec("3000.0000"), dec("1000.0000"), dec("2000.0000"), 2)
        );

        let (g_income, g_spending, _) = graph_totals(&schema, &user, &only_salary, None).await;
        assert_eq!((g_income, g_spending), (dec("3000.0000"), dec("1000.0000")));
    }

    /// (4) A flagged leg with no resolved counterpart stays excluded: we
    /// cannot *prove* the other side is outside the scope, and unknown data
    /// must not flip a figure the user has already looked at.
    #[tokio::test]
    async fn a_flagged_leg_without_a_counterpart_stays_excluded() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let (user, ids) = tenant(&db, "ts-null-counterpart", &["Bucket"]).await;
        let orphan = common::seed_transaction(&db, ids[0], "2024-05-02", "1000.00", Some("Own account")).await;
        flag_without_counterpart(&db, orphan).await;
        common::seed_transaction(&db, ids[0], "2024-05-10", "-50.00", Some("Supermarkt")).await;

        let only = format!("{MAY}, {}", accounts_arg(&ids));
        assert_eq!(
            summary(&schema, &user, &only).await,
            (dec("0.0000"), dec("50.0000"), dec("-50.0000"), 1),
            "an unprovable counterpart keeps the conservative exclusion"
        );
    }

    /// (5) Unchanged: a row whose resolved category is `kind = transfer` is
    /// dropped from the CATEGORY dimension by its own rule, with no
    /// `transaction_insight` row at all and whatever the selection.
    #[tokio::test]
    async fn a_transfer_kind_category_stays_excluded_even_for_one_account() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let (user, ids) = tenant(&db, "ts-kind", &["Salary"]).await;
        let (transfer_cat, transfer_slug) =
            common::seed_category(&db, "transfer.internal", "Internal", "transfer").await;
        let (rent_cat, rent_slug) = common::seed_category(&db, "expense.rent", "Rent", "expense").await;

        common::seed_transaction(&db, ids[0], "2024-05-01", "3000.00", Some("Employer")).await;
        let moved = common::seed_transaction(&db, ids[0], "2024-05-02", "-1000.00", Some("Own account")).await;
        common::seed_transaction_label(&db, moved, Some(transfer_cat), "user", "resolved").await;
        let rent = common::seed_transaction(&db, ids[0], "2024-05-03", "-900.00", Some("Landlord")).await;
        common::seed_transaction_label(&db, rent, Some(rent_cat), "user", "resolved").await;

        let only = format!("{MAY}, {}", accounts_arg(&ids));
        let (_, spending, data) =
            graph_totals(&schema, &user, &only, Some("INCOME_SOURCE, ACCOUNT, CATEGORY")).await;
        assert_eq!(spending, dec("900.0000"), "only the rent is spending");
        let nodes = data["nodes"].as_array().unwrap();
        assert!(
            nodes.iter().any(|n| n["refType"] == "category" && n["refId"] == rent_slug),
            "rent must have a category node: {nodes:?}"
        );
        assert!(
            !nodes.iter().any(|n| n["refId"] == transfer_slug),
            "a transfer-kind category must not appear: {nodes:?}"
        );
    }

    /// (6) `transfer: true` ("show me only transfers") means exactly what it
    /// always did — it is not scope-aware, so a single-account selection
    /// still shows that account's flagged leg.
    #[tokio::test]
    async fn only_transfers_is_unchanged() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let (user, ids) = tenant(&db, "ts-only-transfers", &["Salary", "Bucket"]).await;
        salary_and_bucket(&db, ids[0], ids[1]).await;

        // Household: both legs, nothing else.
        assert_eq!(
            summary(&schema, &user, &format!("{MAY}, transfer: true")).await,
            (dec("1000.0000"), dec("1000.0000"), dec("0.0000"), 2)
        );
        // One account: that account's leg, nothing else.
        let only_bucket = format!("{MAY}, transfer: true, {}", accounts_arg(&ids[1..2]));
        assert_eq!(
            summary(&schema, &user, &only_bucket).await,
            (dec("1000.0000"), dec("0.0000"), dec("1000.0000"), 1)
        );
    }

    /// (7) The case that proves the rule is about the *selection*, not the
    /// authorisation scope: all three accounts are linked to the caller, so
    /// `scoped_ids` always holds all three. With two selected, the transfer
    /// between them is still excluded while the transfer to the unselected
    /// third counts as spending.
    #[tokio::test]
    async fn with_two_of_three_accounts_selected_only_the_inside_transfer_is_excluded() {
        let db = common::db().await;
        let schema = create_schema(db.clone(), common::dummy_settings());
        let (user, ids) = tenant(&db, "ts-three", &["Salary", "Bucket", "Outside"]).await;
        let (a, b, c) = (ids[0], ids[1], ids[2]);

        common::seed_transaction(&db, a, "2024-05-01", "3000.00", Some("Employer")).await;
        let to_b_out = common::seed_transaction(&db, a, "2024-05-02", "-500.00", Some("Own account")).await;
        let to_b_in = common::seed_transaction(&db, b, "2024-05-02", "500.00", Some("Own account")).await;
        pair_as_transfer(&db, to_b_out, to_b_in).await;
        let to_c_out = common::seed_transaction(&db, a, "2024-05-03", "-700.00", Some("Own account")).await;
        let to_c_in = common::seed_transaction(&db, c, "2024-05-03", "700.00", Some("Own account")).await;
        pair_as_transfer(&db, to_c_out, to_c_in).await;

        // Household: neither transfer is spending.
        assert_eq!(
            summary(&schema, &user, MAY).await,
            (dec("3000.0000"), dec("0.0000"), dec("3000.0000"), 1)
        );

        // A and B selected: the A->B pair is inside the selection and stays
        // excluded; A->C leaves the selection and is spending.
        let a_and_b = format!("{MAY}, {}", accounts_arg(&[a, b]));
        assert_eq!(
            summary(&schema, &user, &a_and_b).await,
            (dec("3000.0000"), dec("700.0000"), dec("2300.0000"), 2)
        );
        let (g_income, g_spending, _) = graph_totals(&schema, &user, &a_and_b, None).await;
        assert_eq!((g_income, g_spending), (dec("3000.0000"), dec("700.0000")));

        // The unselected third account on its own: its inbound leg is income.
        let only_c = format!("{MAY}, {}", accounts_arg(&[c]));
        assert_eq!(
            summary(&schema, &user, &only_c).await,
            (dec("700.0000"), dec("0.0000"), dec("700.0000"), 1)
        );
    }
}
