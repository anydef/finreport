//! WP-B (iteration 4 §4) GraphQL integration tests for the goal resolvers,
//! gated behind the `integration` feature. Ownership and validation are
//! checked without a broker (both must fail before any publish); the
//! mutations that publish use a throwaway Kafka container, as in
//! `graphql_iter3_resolvers.rs`.
#![cfg(feature = "integration")]

mod common;

use async_graphql::Request;
use chrono::Utc;
use entity::entities::{goal, transaction_tag};
use rust_decimal::Decimal;
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, RawSessionToken};
use webapp::kafka::goals::TOPIC_GOAL;

fn authed(request: Request, user: &AuthenticatedUser) -> Request {
    request_with_auth(request, Some(user.clone()), RawSessionToken::default())
}

fn caller(user_id: Uuid, username: &str, account: Uuid) -> AuthenticatedUser {
    AuthenticatedUser {
        user_id,
        username: username.to_string(),
        account_ids: vec![account],
    }
}

async fn start_kafka_with_goal_topic(
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
    admin
        .create_topics(
            &[NewTopic::new(TOPIC_GOAL, 1, TopicReplication::Fixed(1))],
            &AdminOptions::new().request_timeout(Some(Duration::from_secs(10))),
        )
        .await
        .expect("create goal topic on Kafka testcontainer");

    (container, bootstrap_servers)
}

fn settings_with_kafka(bootstrap_servers: &str) -> Arc<utils::settings::Settings> {
    let mut settings = (*common::dummy_settings()).clone();
    settings.kafka_brokers = Some(bootstrap_servers.to_string());
    Arc::new(settings)
}

async fn seed_tag(db: &DatabaseConnection, transaction_id: Uuid, tag: &str) {
    transaction_tag::ActiveModel {
        transaction_id: Set(transaction_id),
        tag: Set(tag.to_string()),
        revision: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert transaction_tag");
}

/// Inserts a projected `goal` row directly (as the projector would have), so
/// ownership checks need no broker. A tags-only monthly spending limit.
async fn seed_goal(db: &DatabaseConnection, owner: Uuid, tag: &str) -> Uuid {
    let id = Uuid::new_v4();
    goal::ActiveModel {
        id: Set(id),
        owner_user_id: Set(owner),
        name: Set("Seeded goal".to_string()),
        goal_type: Set("spending_limit".to_string()),
        amount: Set(Decimal::new(20000, 2)),
        currency: Set("EUR".to_string()),
        scope_category_slugs: Set(vec![]),
        scope_tags: Set(vec![tag.to_string()]),
        scope_combine: Set("all".to_string()),
        scope_tag_combine: Set("all".to_string()),
        period_kind: Set("recurring".to_string()),
        period_cadence: Set(Some("monthly".to_string())),
        period_start: Set(None),
        period_end: Set(None),
        archived: Set(false),
        revision: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert goal");
    id
}

fn error_code(err: &async_graphql::ServerError) -> Option<String> {
    err.extensions
        .as_ref()
        .and_then(|e| e.get("code"))
        .map(|v| v.to_string())
}

#[tokio::test]
async fn another_users_goal_is_null_or_denied_everywhere() {
    let db = common::db().await;
    let (owner, _) = common::seed_user(&db, "goal-owner", "pw").await;
    let owner_account = common::seed_account(&db, "EUR", "Goal owner account").await;
    common::link(&db, owner, owner_account).await;
    let (intruder, _) = common::seed_user(&db, "goal-intruder", "pw").await;
    let intruder_account = common::seed_account(&db, "EUR", "Goal intruder account").await;
    common::link(&db, intruder, intruder_account).await;

    let id = seed_goal(&db, owner, "owner-only").await;
    // No broker configured: ownership must be rejected before any publish.
    let schema = create_schema(db.clone(), common::dummy_settings());
    let owner_user = caller(owner, "goal-owner", owner_account);
    let intruder_user = caller(intruder, "goal-intruder", intruder_account);

    // The owner sees it.
    let q = format!(r#"{{ goal(id: "{id}") {{ id name }} }}"#);
    let r = schema.execute(authed(Request::new(q.clone()), &owner_user)).await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.data.into_json().unwrap()["goal"]["id"], id.to_string());

    // The intruder gets null, and an empty list.
    let r = schema.execute(authed(Request::new(q), &intruder_user)).await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert!(r.data.into_json().unwrap()["goal"].is_null());

    let r = schema
        .execute(authed(
            Request::new("{ goals(includeArchived: true) { id } }"),
            &intruder_user,
        ))
        .await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let data = r.data.into_json().unwrap();
    assert!(
        data["goals"].as_array().unwrap().iter().all(|g| g["id"] != id.to_string()),
        "{data}"
    );

    // Everything else is denied with NOT_FOUND.
    let denied = [
        format!(r#"{{ goalProgress(id: "{id}") {{ total }} }}"#),
        format!(
            r#"{{ goalTransactions(id: "{id}", startDate: "2024-01-01", endDate: "2024-12-31") {{ totalCount }} }}"#
        ),
        format!(r#"mutation {{ archiveGoal(id: "{id}") {{ id }} }}"#),
        format!(
            r#"mutation {{ updateGoal(id: "{id}", input: {{ name: "x", type: SPENDING_LIMIT, amount: "1",
                categorySlugs: [], tags: ["x"], periodKind: RECURRING, cadence: MONTHLY }}) {{ id }} }}"#
        ),
    ];
    for query in denied {
        let r = schema.execute(authed(Request::new(query.clone()), &intruder_user)).await;
        assert_eq!(r.errors.len(), 1, "expected a denial for {query}: {:?}", r.errors);
        assert_eq!(error_code(&r.errors[0]).as_deref(), Some("\"NOT_FOUND\""), "{query}");
    }
}

#[tokio::test]
async fn goal_transactions_are_restricted_to_the_callers_accounts() {
    let db = common::db().await;
    let (user_a, _) = common::seed_user(&db, "goal-tx-a", "pw").await;
    let account_a = common::seed_account(&db, "EUR", "Goal tx A").await;
    common::link(&db, user_a, account_a).await;
    let (user_b, _) = common::seed_user(&db, "goal-tx-b", "pw").await;
    let account_b = common::seed_account(&db, "EUR", "Goal tx B").await;
    common::link(&db, user_b, account_b).await;

    let tag = format!("shared-{}", Uuid::new_v4().simple());
    let mut mine = Vec::new();
    for day in ["2024-07-01", "2024-07-02", "2024-07-03"] {
        let tx = common::seed_transaction(&db, account_a, day, "-10.00", Some("Mine")).await;
        seed_tag(&db, tx, &tag).await;
        mine.push(tx);
    }
    let theirs = common::seed_transaction(&db, account_b, "2024-07-02", "-10.00", Some("Theirs")).await;
    seed_tag(&db, theirs, &tag).await;

    let id = seed_goal(&db, user_a, &tag).await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller_a = caller(user_a, "goal-tx-a", account_a);

    let query = format!(
        r#"{{ goalTransactions(id: "{id}", startDate: "2024-07-01", endDate: "2024-07-31") {{
            totalCount items {{ id accountId }} }} }}"#
    );
    let r = schema.execute(authed(Request::new(query), &caller_a)).await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let page = &r.data.into_json().unwrap()["goalTransactions"];
    assert_eq!(page["totalCount"], 3);
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    for item in items {
        assert_eq!(item["accountId"], account_a.to_string());
        assert_ne!(item["id"], theirs.to_string());
    }
    // Newest first.
    assert_eq!(items[0]["id"], mine[2].to_string());

    // Paging slices the same list; an oversized limit is clamped to 200.
    let paged = format!(
        r#"{{ goalTransactions(id: "{id}", startDate: "2024-07-01", endDate: "2024-07-31",
            page: {{ limit: 1000, offset: 2 }}) {{ totalCount limit offset items {{ id }} }} }}"#
    );
    let r = schema.execute(authed(Request::new(paged), &caller_a)).await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let page = &r.data.into_json().unwrap()["goalTransactions"];
    assert_eq!(page["limit"], 200);
    assert_eq!(page["offset"], 2);
    assert_eq!(page["totalCount"], 3);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);

    // A reversed range is a validation error.
    let reversed = format!(
        r#"{{ goalTransactions(id: "{id}", startDate: "2024-08-01", endDate: "2024-07-01") {{ totalCount }} }}"#
    );
    let r = schema.execute(authed(Request::new(reversed), &caller_a)).await;
    assert_eq!(error_code(&r.errors[0]).as_deref(), Some("\"VALIDATION\""));
}

#[tokio::test]
async fn create_goal_rejects_each_bad_input_shape_before_publishing() {
    let db = common::db().await;
    let (user, _) = common::seed_user(&db, "goal-validation", "pw").await;
    let account = common::seed_account(&db, "EUR", "Goal validation account").await;
    common::link(&db, user, account).await;
    let (_, slug) = common::seed_category(&db, "expense.goal-val", "Hobbies", "EXPENSE").await;
    // No broker: any input that got past validation would fail with
    // KAFKA_UNAVAILABLE instead, so VALIDATION proves the order.
    let schema = create_schema(db.clone(), common::dummy_settings());
    let me = caller(user, "goal-validation", account);

    let recurring = r#"periodKind: RECURRING, cadence: MONTHLY"#;
    let bad: Vec<(&str, String)> = vec![
        ("zero amount", format!(r#"name: "g", type: SPENDING_LIMIT, amount: "0", categorySlugs: ["{slug}"], {recurring}"#)),
        ("negative amount", format!(r#"name: "g", type: SPENDING_LIMIT, amount: "-5", categorySlugs: ["{slug}"], {recurring}"#)),
        ("empty scope", format!(r#"name: "g", type: SPENDING_LIMIT, amount: "10", categorySlugs: [], tags: [], {recurring}"#)),
        ("recurring without cadence", format!(r#"name: "g", type: SPENDING_LIMIT, amount: "10", categorySlugs: ["{slug}"], periodKind: RECURRING"#)),
        ("recurring with startDate", format!(r#"name: "g", type: SPENDING_LIMIT, amount: "10", categorySlugs: ["{slug}"], {recurring}, startDate: "2027-01-01""#)),
        ("recurring with endDate", format!(r#"name: "g", type: SPENDING_LIMIT, amount: "10", categorySlugs: ["{slug}"], {recurring}, endDate: "2027-01-31""#)),
        ("fixed without startDate", format!(r#"name: "g", type: SPENDING_LIMIT, amount: "10", categorySlugs: ["{slug}"], periodKind: FIXED"#)),
        ("fixed with cadence", format!(r#"name: "g", type: SPENDING_LIMIT, amount: "10", categorySlugs: ["{slug}"], periodKind: FIXED, startDate: "2027-01-01", cadence: MONTHLY"#)),
        ("end before start", format!(r#"name: "g", type: SPENDING_LIMIT, amount: "10", categorySlugs: ["{slug}"], periodKind: FIXED, startDate: "2027-02-01", endDate: "2027-01-01""#)),
        ("unknown category", format!(r#"name: "g", type: SPENDING_LIMIT, amount: "10", categorySlugs: ["no.such.category"], {recurring}"#)),
        ("blank name", format!(r#"name: "  ", type: SPENDING_LIMIT, amount: "10", categorySlugs: ["{slug}"], {recurring}"#)),
    ];
    for (label, input) in bad {
        let query = format!("mutation {{ createGoal(input: {{ {input} }}) {{ id }} }}");
        let r = schema.execute(authed(Request::new(query), &me)).await;
        assert_eq!(r.errors.len(), 1, "{label}: {:?}", r.errors);
        assert_eq!(error_code(&r.errors[0]).as_deref(), Some("\"VALIDATION\""), "{label}: {}", r.errors[0].message);
    }

    // The same call, valid, reaches the publish step and (with no broker)
    // stops there rather than at validation.
    let ok = format!(
        r#"mutation {{ createGoal(input: {{ name: "g", type: SPENDING_LIMIT, amount: "10", categorySlugs: ["{slug}"], {recurring} }}) {{ id }} }}"#
    );
    let r = schema.execute(authed(Request::new(ok), &me)).await;
    assert_eq!(error_code(&r.errors[0]).as_deref(), Some("\"KAFKA_UNAVAILABLE\""));
}

#[tokio::test]
async fn update_goal_preserves_what_the_input_does_not_carry() {
    let (_kafka, bootstrap) = start_kafka_with_goal_topic().await;
    let db = common::db().await;
    let (user, _) = common::seed_user(&db, "goal-update", "pw").await;
    let account = common::seed_account(&db, "EUR", "Goal update account").await;
    common::link(&db, user, account).await;
    let (_, slug) = common::seed_category(&db, "expense.goal-upd", "Hobbies", "EXPENSE").await;
    let schema = create_schema(db.clone(), settings_with_kafka(&bootstrap));
    let me = caller(user, "goal-update", account);

    let create = format!(
        r#"mutation {{ createGoal(input: {{ name: "Hobbies", type: SPENDING_LIMIT, amount: "200",
            categorySlugs: ["{slug}"], tags: ["Hobby"], combine: ANY, periodKind: RECURRING, cadence: MONTHLY }}) {{
            id name amount archived scope {{ tags combine tagCombine categories {{ slug }} }} }} }}"#
    );
    let r = schema.execute(authed(Request::new(create), &me)).await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let created = r.data.into_json().unwrap()["createGoal"].clone();
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["archived"], false);
    assert_eq!(created["scope"]["tags"][0], "hobby", "tags are normalised");
    assert_eq!(created["scope"]["categories"][0]["slug"], slug);

    // Archive: the record is rewritten, not tombstoned.
    let archive = format!(r#"mutation {{ archiveGoal(id: "{id}") {{ archived name }} }}"#);
    let r = schema.execute(authed(Request::new(archive), &me)).await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let archived = &r.data.into_json().unwrap()["archiveGoal"];
    assert_eq!(archived["archived"], true);
    assert_eq!(archived["name"], "Hobbies");

    let hidden = schema
        .execute(authed(Request::new("{ goals { id } }"), &me))
        .await
        .data
        .into_json()
        .unwrap();
    assert!(hidden["goals"].as_array().unwrap().iter().all(|g| g["id"] != id));
    let shown = schema
        .execute(authed(Request::new("{ goals(includeArchived: true) { id archived } }"), &me))
        .await
        .data
        .into_json()
        .unwrap();
    assert!(shown["goals"].as_array().unwrap().iter().any(|g| g["id"] == id && g["archived"] == true));

    // Update: change the amount and cadence; `archived` (not in the input)
    // must survive, and so must the owner.
    let update = format!(
        r#"mutation {{ updateGoal(id: "{id}", input: {{ name: "Hobbies 2", type: SPENDING_LIMIT, amount: "250.5",
            categorySlugs: ["{slug}"], tags: ["hobby"], combine: ANY, periodKind: RECURRING, cadence: QUARTERLY }}) {{
            name amount archived cadence scope {{ tags combine }} }} }}"#
    );
    let r = schema.execute(authed(Request::new(update), &me)).await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let updated = &r.data.into_json().unwrap()["updateGoal"];
    assert_eq!(updated["name"], "Hobbies 2");
    assert_eq!(updated["amount"], "250.5000");
    assert_eq!(updated["cadence"], "QUARTERLY");
    assert_eq!(updated["scope"]["combine"], "ANY");
    assert_eq!(updated["archived"], true, "updateGoal must not un-archive");

    let row = goal::Entity::find_by_id(id.parse::<Uuid>().unwrap())
        .one(db.as_ref())
        .await
        .unwrap()
        .expect("goal row");
    assert_eq!(row.owner_user_id, user);
}
