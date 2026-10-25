//! The admin gate (`AdminOnly`): the resolvers that change or expose shared
//! labeling state (categories, rules, learning exemptions) are administrator
//! only, everything that works on the caller's own data stays open.
//!
//! Gated: createCategory, renameCategory, archiveCategory, createRule,
//! updateRule, setRuleState, reapplyRule, exemptFromLearning,
//! removeLearningExemption, and the queries rules, recentlyAutoApprovedRules,
//! learningExemptions. Deliberately open: ensureCategory (the review flow).
#![cfg(feature = "integration")]

mod common;

use async_graphql::{Request, Response};
use chrono::Utc;
use entity::entities::{category, learning_exemption, rule};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, AppSchema, RawSessionToken};
use webapp::kafka::labeling::{
    TOPIC_CATEGORY, TOPIC_DISPLAY_ALIAS, TOPIC_LABEL_REQUEST, TOPIC_LEARNING_EXEMPTION, TOPIC_RULE,
    TOPIC_USER_LABEL,
};

type KafkaContainer = testcontainers::ContainerAsync<testcontainers_modules::kafka::apache::Kafka>;

async fn start_kafka() -> (KafkaContainer, String) {
    use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
    use rdkafka::client::DefaultClientContext;
    use rdkafka::ClientConfig;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::kafka::apache::{Kafka, KAFKA_PORT};

    let container = Kafka::default().start().await.expect("start Kafka testcontainer");
    let host = container.get_host().await.expect("Kafka host");
    let port = container.get_host_port_ipv4(KAFKA_PORT).await.expect("Kafka port");
    let bootstrap = format!("{host}:{port}");
    let admin: AdminClient<DefaultClientContext> =
        ClientConfig::new().set("bootstrap.servers", &bootstrap).create().expect("admin client");
    let topics = [
        TOPIC_USER_LABEL,
        TOPIC_RULE,
        TOPIC_CATEGORY,
        TOPIC_LABEL_REQUEST,
        TOPIC_LEARNING_EXEMPTION,
        TOPIC_DISPLAY_ALIAS,
    ]
    .map(|name| NewTopic::new(name, 1, TopicReplication::Fixed(1)));
    admin
        .create_topics(&topics, &AdminOptions::new().request_timeout(Some(Duration::from_secs(10))))
        .await
        .expect("create topics");
    (container, bootstrap)
}

fn schema(db: &Arc<DatabaseConnection>, bootstrap: Option<&str>) -> AppSchema {
    let mut settings = (*common::dummy_settings()).clone();
    settings.kafka_brokers = bootstrap.map(str::to_string);
    create_schema(db.clone(), Arc::new(settings))
}

struct Caller {
    user: AuthenticatedUser,
    account: Uuid,
}

async fn caller(db: &DatabaseConnection, name: &str, admin: bool) -> Caller {
    let (user_id, username) = if admin {
        common::seed_admin(db, name, "pw").await
    } else {
        common::seed_user(db, name, "pw").await
    };
    let account = common::seed_account(db, "EUR", name).await;
    common::link(db, user_id, account).await;
    Caller { user: AuthenticatedUser { user_id, username, account_ids: vec![account] }, account }
}

async fn exec(schema: &AppSchema, user: Option<&AuthenticatedUser>, query: &str) -> Response {
    schema
        .execute(request_with_auth(Request::new(query), user.cloned(), RawSessionToken::default()))
        .await
}

fn code(response: &Response) -> Option<String> {
    response
        .errors
        .first()
        .and_then(|e| e.extensions.as_ref())
        .and_then(|x| x.get("code"))
        .map(|c| c.to_string().trim_matches('"').to_string())
}

async fn seed_rule(db: &DatabaseConnection, category: Uuid) -> rule::Model {
    rule::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set("gate test".to_string()),
        category_id: Set(category),
        conditions: Set(serde_json::json!({ "description_contains": "gate" })),
        priority: Set(0),
        state: Set("ACTIVE".to_string()),
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
    .expect("insert rule")
}

async fn seed_exemption(db: &DatabaseConnection) -> learning_exemption::Model {
    learning_exemption::ActiveModel {
        counterparty_key: Set(format!("gate-{}", Uuid::new_v4().simple())),
        decided_by: Set(None),
        revision: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert learning_exemption")
}

/// The names a refused attempt would have brought into existence, so the
/// test can assert their *absence* instead of comparing whole tables.
struct Attempted {
    category_slug: String,
    exemption_key: String,
    rule_name: &'static str,
}

/// Every gated operation, with arguments that would succeed for an admin.
fn gated_operations(
    cat: &category::Model,
    rule: &rule::Model,
    exemption_key: &str,
) -> (Vec<(&'static str, String)>, Attempted) {
    let new_slug = format!("gate_new_{}", Uuid::new_v4().simple());
    let rule_input = format!(
        r#"{{ name: "gate created", categorySlug: "{}", conditions: {{ description_contains: "gate" }}, priority: 1 }}"#,
        cat.slug
    );
    let operations = vec![
        ("createCategory", format!(r#"mutation {{ createCategory(input: {{ slug: "{new_slug}", name: "N", kind: EXPENSE }}) {{ id }} }}"#)),
        ("renameCategory", format!(r#"mutation {{ renameCategory(id: "{}", name: "Renamed") {{ id }} }}"#, cat.id)),
        ("archiveCategory", format!(r#"mutation {{ archiveCategory(id: "{}") {{ id }} }}"#, cat.id)),
        ("createRule", format!("mutation {{ createRule(input: {rule_input}) {{ id }} }}")),
        ("updateRule", format!(r#"mutation {{ updateRule(id: "{}", input: {rule_input}) {{ id }} }}"#, rule.id)),
        ("setRuleState", format!(r#"mutation {{ setRuleState(id: "{}", state: REVOKED) {{ id }} }}"#, rule.id)),
        ("reapplyRule", format!(r#"mutation {{ reapplyRule(id: "{}") }}"#, rule.id)),
        ("exemptFromLearning", format!(r#"mutation {{ exemptFromLearning(counterpartyKey: "gate-new-{new_slug}") {{ counterpartyKey }} }}"#)),
        ("removeLearningExemption", format!(r#"mutation {{ removeLearningExemption(counterpartyKey: "{exemption_key}") }}"#)),
        ("rules", "{ rules { id } }".to_string()),
        ("recentlyAutoApprovedRules", "{ recentlyAutoApprovedRules { id } }".to_string()),
        ("learningExemptions", "{ learningExemptions { counterpartyKey } }".to_string()),
    ];
    let attempted = Attempted {
        category_slug: new_slug.clone(),
        exemption_key: format!("gate-new-{new_slug}"),
        rule_name: "gate created",
    };
    (operations, attempted)
}

#[tokio::test]
async fn a_non_admin_is_forbidden_on_every_gated_field_and_nothing_changes() {
    let (_kafka, bootstrap) = start_kafka().await;
    let db = common::db().await;
    let schema = schema(&db, Some(&bootstrap));
    let nobody = caller(&db, "gate-plain", false).await;
    let (cat_id, _) = common::seed_category(&db, "gate", "Gate", "EXPENSE").await;
    let cat = category::Entity::find_by_id(cat_id).one(db.as_ref()).await.unwrap().unwrap();
    let rule = seed_rule(&db, cat_id).await;
    let exemption = seed_exemption(&db).await;

    // Only the rows this test owns are snapshotted. The test database is
    // shared and sibling tests in this very binary create categories
    // concurrently, so comparing whole tables made this fail about half the
    // time under the default parallelism - a flaky security test, which is
    // worse than none because it teaches the reader to rerun. Per-row
    // assertions are also stronger: they pin that nothing was renamed,
    // archived, revoked or deleted, not merely that a count held.
    let (operations, attempted) = gated_operations(&cat, &rule, &exemption.counterparty_key);
    assert_eq!(operations.len(), 12, "all nine mutations and three queries");
    for (name, query) in &operations {
        let response = exec(&schema, Some(&nobody.user), query).await;
        assert_eq!(code(&response).as_deref(), Some("FORBIDDEN"), "{name}: {:?}", response.errors);
    }

    // Nothing the refusal touched changed.
    assert_eq!(
        Some(cat.clone()),
        category::Entity::find_by_id(cat.id).one(db.as_ref()).await.unwrap(),
        "the targeted category was renamed or archived"
    );
    assert_eq!(
        Some(rule.clone()),
        rule::Entity::find_by_id(rule.id).one(db.as_ref()).await.unwrap(),
        "the targeted rule was updated or revoked"
    );
    assert!(
        learning_exemption::Entity::find_by_id(exemption.counterparty_key.clone())
            .one(db.as_ref())
            .await
            .unwrap()
            .is_some(),
        "the targeted exemption was deleted"
    );

    // And nothing the refusal would have created exists.
    assert!(
        category::Entity::find()
            .filter(category::Column::Slug.eq(attempted.category_slug.clone()))
            .one(db.as_ref())
            .await
            .unwrap()
            .is_none(),
        "createCategory wrote despite being refused"
    );
    assert!(
        rule::Entity::find()
            .filter(rule::Column::Name.eq(attempted.rule_name))
            .one(db.as_ref())
            .await
            .unwrap()
            .is_none(),
        "createRule wrote despite being refused"
    );
    assert!(
        learning_exemption::Entity::find_by_id(attempted.exemption_key.clone())
            .one(db.as_ref())
            .await
            .unwrap()
            .is_none(),
        "exemptFromLearning wrote despite being refused"
    );
}

#[tokio::test]
async fn an_unauthenticated_caller_still_gets_unauthenticated_not_forbidden() {
    let db = common::db().await;
    let schema = schema(&db, None);
    let (cat_id, _) = common::seed_category(&db, "gate_anon", "Anon", "EXPENSE").await;
    let cat = category::Entity::find_by_id(cat_id).one(db.as_ref()).await.unwrap().unwrap();
    let rule = seed_rule(&db, cat_id).await;
    for (name, query) in gated_operations(&cat, &rule, "k").0 {
        let response = exec(&schema, None, &query).await;
        assert_eq!(code(&response).as_deref(), Some("UNAUTHENTICATED"), "{name}: {:?}", response.errors);
    }
}

#[tokio::test]
async fn an_admin_passes_the_gate() {
    let db = common::db().await;
    // No broker: reads work, and a mutation gets past the gate to the
    // publisher check, which is the next thing it hits.
    let schema = schema(&db, None);
    let admin = caller(&db, "gate-admin", true).await;
    for query in [
        "{ rules { id } }",
        "{ recentlyAutoApprovedRules { id } }",
        "{ learningExemptions { counterpartyKey } }",
    ] {
        let response = exec(&schema, Some(&admin.user), query).await;
        assert!(response.errors.is_empty(), "{query}: {:?}", response.errors);
    }
    let slug = format!("gate_admin_{}", Uuid::new_v4().simple());
    let response = exec(
        &schema,
        Some(&admin.user),
        &format!(r#"mutation {{ createCategory(input: {{ slug: "{slug}", name: "A", kind: EXPENSE }}) {{ id }} }}"#),
    )
    .await;
    assert_ne!(code(&response).as_deref(), Some("FORBIDDEN"), "{:?}", response.errors);
    assert_ne!(code(&response).as_deref(), Some("UNAUTHENTICATED"), "{:?}", response.errors);
}

#[tokio::test]
async fn the_flag_is_read_from_the_database_on_every_call() {
    let db = common::db().await;
    let schema = schema(&db, None);
    let admin = caller(&db, "gate-demoted", true).await;
    let query = "{ rules { id } }";
    assert!(exec(&schema, Some(&admin.user), query).await.errors.is_empty());

    db.execute_unprepared(&format!(
        "UPDATE app_user SET is_admin = false WHERE id = '{}'",
        admin.user.user_id
    ))
    .await
    .unwrap();
    let response = exec(&schema, Some(&admin.user), query).await;
    assert_eq!(code(&response).as_deref(), Some("FORBIDDEN"), "{:?}", response.errors);
}

#[tokio::test]
async fn a_non_admin_can_still_do_their_own_work() {
    let (_kafka, bootstrap) = start_kafka().await;
    let db = common::db().await;
    let schema = schema(&db, Some(&bootstrap));
    let user = caller(&db, "gate-curator", false).await;
    let tx = common::seed_transaction(&db, user.account, "2024-07-01", "-12.00", Some("Gate Shop")).await;

    // The review flow: ensure the proposed category exists, then assign it.
    let slug = format!("gate_proposed_{}", Uuid::new_v4().simple());
    let ensured = exec(
        &schema,
        Some(&user.user),
        &format!(r#"mutation {{ ensureCategory(input: {{ slug: "{slug}", name: "Proposed", kind: EXPENSE }}) {{ id slug }} }}"#),
    )
    .await;
    assert!(ensured.errors.is_empty(), "ensureCategory must stay open: {:?}", ensured.errors);

    let assigned = exec(
        &schema,
        Some(&user.user),
        &format!(r#"mutation {{ setTransactionCategory(transactionId: "{tx}", categorySlug: "{slug}") {{ id }} }}"#),
    )
    .await;
    assert!(assigned.errors.is_empty(), "categorising must stay open: {:?}", assigned.errors);

    let aliased = exec(
        &schema,
        Some(&user.user),
        &format!(r#"mutation {{ setDisplayAlias(kind: ACCOUNT, key: "{}", alias: "Mine") {{ alias }} }}"#, user.account),
    )
    .await;
    assert!(aliased.errors.is_empty(), "display aliases must stay open: {:?}", aliased.errors);
}
