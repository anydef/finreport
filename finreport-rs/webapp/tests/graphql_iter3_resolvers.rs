//! WP-B (iteration 3 §4) GraphQL-level integration tests, gated behind the
//! `integration` feature, mirroring `graphql_integration.rs`'s fixtures and
//! patterns. These tests seed the WP0 read-model tables
//! (`transaction_tag`, `transaction_insight`, `transaction_user_label`)
//! directly — the detector/projection work packages aren't on this branch,
//! per the task's instructions.
#![cfg(feature = "integration")]

mod common;

use async_graphql::Request;
use chrono::Utc;
use entity::entities::{transaction_insight, transaction_tag, transaction_user_label};
use rust_decimal::Decimal;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, RawSessionToken};
use webapp::kafka::labeling::{TOPIC_CATEGORY, TOPIC_LABEL_REQUEST, TOPIC_RULE, TOPIC_USER_LABEL};

fn authed(request: Request, user: Option<AuthenticatedUser>) -> Request {
    request_with_auth(request, user, RawSessionToken::default())
}

/// Same throwaway-Kafka-container helper as `graphql_integration.rs` (not
/// shared via `common` since that module is outside WP-B's owned
/// `webapp/src/graphql/**` + `graphql_iter3_*.rs` scope).
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

/// Seeds a `transaction_tag` row directly (as WP0's projector would have
/// applied from a `finreport.user-label` record).
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

/// Seeds a `transaction_insight` row (the detector's read model) with
/// sensible non-transfer, non-recurring defaults, overridable per field.
#[allow(clippy::too_many_arguments)]
async fn seed_insight(
    db: &DatabaseConnection,
    transaction_id: Uuid,
    is_transfer: bool,
    transfer_counterpart_id: Option<Uuid>,
    is_recurring: bool,
    recurring_series_id: Option<Uuid>,
    recurring_cadence: Option<&str>,
    recurring_median_amount: Option<&str>,
) {
    transaction_insight::ActiveModel {
        transaction_id: Set(transaction_id),
        is_transfer: Set(is_transfer),
        transfer_counterpart_id: Set(transfer_counterpart_id),
        transfer_match: Set(if is_transfer { Some("amount_date".to_string()) } else { None }),
        is_recurring: Set(is_recurring),
        recurring_series_id: Set(recurring_series_id),
        recurring_cadence: Set(recurring_cadence.map(str::to_string)),
        recurring_median_amount: Set(recurring_median_amount.map(|a| Decimal::from_str(a).unwrap())),
        detected_at: Set(Utc::now().into()),
        revision: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert transaction_insight");
}

/// Seeds a `transaction_user_label` row with only `recurring` set (no
/// category/note), as `setTransactionRecurring` would leave it absent a
/// prior category mutation.
async fn seed_user_label_recurring(db: &DatabaseConnection, transaction_id: Uuid, recurring: Option<bool>) {
    transaction_user_label::ActiveModel {
        transaction_id: Set(transaction_id),
        category_id: Set(None),
        note: Set(None),
        revision: Set(Utc::now().into()),
        recurring: Set(recurring),
    }
    .insert(db)
    .await
    .expect("insert transaction_user_label");
}

#[tokio::test]
async fn transaction_tags_transfer_and_recurring_reflect_the_read_model() {
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "tag-fields-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Tag fields account").await;
    common::link(&db, user, account).await;

    let tx = common::seed_transaction(&db, account, "2024-07-01", "-9.99", Some("Streamflix")).await;
    seed_tag(&db, tx, "subscriptions").await;
    seed_tag(&db, tx, "entertainment").await;

    let counterpart_account = common::seed_account(&db, "EUR", "Counterpart account").await;
    let counterpart_tx =
        common::seed_transaction(&db, counterpart_account, "2024-07-01", "9.99", Some("Me")).await;
    seed_insight(&db, tx, true, Some(counterpart_tx), false, None, None, None).await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "tag-fields-test".to_string(),
        account_ids: vec![account],
    };

    let query = format!(
        r#"{{ transactions(filter: {{ startDate: "2024-07-01", endDate: "2024-07-01" }}) {{ items {{
            id
            tags
            transfer {{ counterpartTransactionId match }}
            recurring {{ isRecurring source }}
        }} }} }}"#
    );
    let response = schema.execute(authed(Request::new(query), Some(caller))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let items = data["transactions"]["items"].as_array().unwrap();
    let t = items
        .iter()
        .find(|t| t["id"] == tx.to_string())
        .unwrap_or_else(|| panic!("seeded transaction not found among {items:?}"));
    let mut tags: Vec<&str> = t["tags"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    tags.sort();
    assert_eq!(tags, vec!["entertainment", "subscriptions"]);
    assert_eq!(t["transfer"]["counterpartTransactionId"], counterpart_tx.to_string());
    assert_eq!(t["recurring"]["isRecurring"], false);
    assert_eq!(t["recurring"]["source"], "AUTO");
}

#[tokio::test]
async fn recurring_user_override_wins_and_clears_series_fields() {
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "recurring-override-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Override account").await;
    common::link(&db, user, account).await;

    let tx = common::seed_transaction(&db, account, "2024-07-02", "-15.00", Some("Gym")).await;
    let series_id = Uuid::new_v4();
    seed_insight(
        &db,
        tx,
        false,
        None,
        true,
        Some(series_id),
        Some("monthly"),
        Some("-15.00"),
    )
    .await;
    // User overrides the detector's "recurring" verdict to false.
    seed_user_label_recurring(&db, tx, Some(false)).await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "recurring-override-test".to_string(),
        account_ids: vec![account],
    };

    let query = format!(
        r#"{{ transactions(filter: {{ startDate: "2024-07-02", endDate: "2024-07-02" }}) {{ items {{
            id recurring {{ isRecurring source seriesId cadence medianAmount }}
        }} }} }}"#
    );
    let response = schema.execute(authed(Request::new(query), Some(caller))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let items = data["transactions"]["items"].as_array().unwrap();
    let recurring = &items
        .iter()
        .find(|t| t["id"] == tx.to_string())
        .unwrap_or_else(|| panic!("seeded transaction not found among {items:?}"))["recurring"];
    assert_eq!(recurring["isRecurring"], false);
    assert_eq!(recurring["source"], "USER");
    assert!(recurring["seriesId"].is_null(), "seriesId must be severed by an override: {recurring:?}");
    assert!(recurring["cadence"].is_null());
    assert!(recurring["medianAmount"].is_null());
}

#[tokio::test]
async fn query_tags_counts_only_the_callers_own_transactions() {
    let db = common::db().await;

    let (user_a, _) = common::seed_user(&db, "tagcount-a-test", "pw").await;
    let account_a = common::seed_account(&db, "EUR", "A's account").await;
    common::link(&db, user_a, account_a).await;
    let tx_a1 = common::seed_transaction(&db, account_a, "2024-07-03", "-5.00", Some("Shop")).await;
    let tx_a2 = common::seed_transaction(&db, account_a, "2024-07-04", "-6.00", Some("Shop")).await;
    seed_tag(&db, tx_a1, "groceries").await;
    seed_tag(&db, tx_a2, "groceries").await;

    let (_user_b, _) = common::seed_user(&db, "tagcount-b-test", "pw").await;
    let account_b = common::seed_account(&db, "EUR", "B's account").await;
    let tx_b = common::seed_transaction(&db, account_b, "2024-07-03", "-5.00", Some("Shop")).await;
    seed_tag(&db, tx_b, "groceries").await;
    seed_tag(&db, tx_b, "only-bs-tag").await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user_a,
        username: "tagcount-a-test".to_string(),
        account_ids: vec![account_a],
    };

    let response = schema
        .execute(authed(Request::new("{ tags { tag transactionCount } }"), Some(caller)))
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let rows = data["tags"].as_array().unwrap();
    assert!(rows.iter().all(|r| r["tag"] != "only-bs-tag"), "must not see B's tag: {rows:?}");
    let groceries = rows.iter().find(|r| r["tag"] == "groceries").expect("groceries row");
    assert_eq!(groceries["transactionCount"], 2);
}

#[tokio::test]
async fn set_transaction_tags_round_trips_and_preserves_category_on_replay() {
    use entity::entities::category;
    use sea_orm::EntityTrait;

    let (_kafka, bootstrap) = start_kafka_with_labeling_topics().await;
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "set-tags-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Tags mutation account").await;
    common::link(&db, user, account).await;
    let tx = common::seed_transaction(&db, account, "2024-07-05", "-20.00", Some("Shop")).await;
    let (category_id, slug) = common::seed_category(&db, "expense.tags-mut", "Groceries", "EXPENSE").await;

    let schema = create_schema(db.clone(), settings_with_kafka(&bootstrap));
    let caller = AuthenticatedUser {
        user_id: user,
        username: "set-tags-test".to_string(),
        account_ids: vec![account],
    };

    // First set a category via the existing iteration-2 mutation.
    let category_query =
        format!(r#"mutation {{ setTransactionCategory(transactionId: "{tx}", categorySlug: "{slug}") {{ id }} }}"#);
    let category_response = schema
        .execute(authed(Request::new(category_query), Some(caller.clone())))
        .await;
    assert!(category_response.errors.is_empty(), "{:?}", category_response.errors);

    // Now set tags; this must NOT clear the category set above (read-modify-write, §2.1).
    // `label`/`transaction_label` is a separate WP3 projection this branch
    // doesn't run, so assert preservation directly against
    // `transaction_user_label.category_id` instead of the `label` field.
    let tags_query = format!(
        r#"mutation {{ setTransactionTags(transactionId: "{tx}", tags: ["Travel", "travel", " Food "]) {{
            tags
        }} }}"#
    );
    let tags_response = schema
        .execute(authed(Request::new(tags_query), Some(caller.clone())))
        .await;
    assert!(tags_response.errors.is_empty(), "{:?}", tags_response.errors);
    let data = tags_response.data.into_json().unwrap();
    let mut tags: Vec<&str> = data["setTransactionTags"]["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    tags.sort();
    // "Travel"/"travel" normalize-dedupe to one tag.
    assert_eq!(tags, vec!["food", "travel"]);

    let user_label = transaction_user_label::Entity::find_by_id(tx)
        .one(db.as_ref())
        .await
        .unwrap()
        .expect("transaction_user_label row");
    assert_eq!(
        user_label.category_id,
        Some(category_id),
        "setTransactionTags must not clear the previously-set category"
    );
    let category = category::Entity::find_by_id(category_id).one(db.as_ref()).await.unwrap().unwrap();
    assert_eq!(category.slug, slug);

    // Replacing again drops the previous set entirely (not additive).
    let replace_query = format!(
        r#"mutation {{ setTransactionTags(transactionId: "{tx}", tags: ["urgent"]) {{ tags }} }}"#
    );
    let replace_response = schema.execute(authed(Request::new(replace_query), Some(caller))).await;
    assert!(replace_response.errors.is_empty(), "{:?}", replace_response.errors);
    let replace_data = replace_response.data.into_json().unwrap();
    assert_eq!(
        replace_data["setTransactionTags"]["tags"].as_array().unwrap(),
        &vec![serde_json::Value::String("urgent".to_string())]
    );
}


#[tokio::test]
async fn set_transaction_recurring_overrides_then_null_restores_auto() {
    let (_kafka, bootstrap) = start_kafka_with_labeling_topics().await;
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "set-recurring-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Recurring mutation account").await;
    common::link(&db, user, account).await;
    let tx = common::seed_transaction(&db, account, "2024-07-06", "-12.00", Some("Gym")).await;
    let series_id = Uuid::new_v4();
    seed_insight(&db, tx, false, None, true, Some(series_id), Some("monthly"), Some("-12.00")).await;

    let schema = create_schema(db.clone(), settings_with_kafka(&bootstrap));
    let caller = AuthenticatedUser {
        user_id: user,
        username: "set-recurring-test".to_string(),
        account_ids: vec![account],
    };

    // Override to false.
    let override_query = format!(
        r#"mutation {{ setTransactionRecurring(transactionId: "{tx}", recurring: false) {{
            recurring {{ isRecurring source seriesId }}
        }} }}"#
    );
    let override_response = schema
        .execute(authed(Request::new(override_query), Some(caller.clone())))
        .await;
    assert!(override_response.errors.is_empty(), "{:?}", override_response.errors);
    let override_data = override_response.data.into_json().unwrap();
    assert_eq!(override_data["setTransactionRecurring"]["recurring"]["isRecurring"], false);
    assert_eq!(override_data["setTransactionRecurring"]["recurring"]["source"], "USER");
    assert!(override_data["setTransactionRecurring"]["recurring"]["seriesId"].is_null());

    // `recurring: null` restores the detector's verdict.
    let restore_query = format!(
        r#"mutation {{ setTransactionRecurring(transactionId: "{tx}", recurring: null) {{
            recurring {{ isRecurring source seriesId }}
        }} }}"#
    );
    let restore_response = schema.execute(authed(Request::new(restore_query), Some(caller))).await;
    assert!(restore_response.errors.is_empty(), "{:?}", restore_response.errors);
    let restore_data = restore_response.data.into_json().unwrap();
    assert_eq!(restore_data["setTransactionRecurring"]["recurring"]["isRecurring"], true);
    assert_eq!(restore_data["setTransactionRecurring"]["recurring"]["source"], "AUTO");
    assert_eq!(
        restore_data["setTransactionRecurring"]["recurring"]["seriesId"],
        series_id.to_string()
    );
}

#[tokio::test]
async fn cross_user_tag_and_recurring_mutations_are_denied() {
    let db = common::db().await;

    let (user_a, _) = common::seed_user(&db, "owner-mut-test", "pw").await;
    let account_a = common::seed_account(&db, "EUR", "Owner account").await;
    common::link(&db, user_a, account_a).await;
    let tx = common::seed_transaction(&db, account_a, "2024-07-07", "-5.00", Some("Shop")).await;

    let (user_b, _) = common::seed_user(&db, "intruder-mut-test", "pw").await;
    let account_b = common::seed_account(&db, "EUR", "Intruder account").await;
    common::link(&db, user_b, account_b).await;

    // No kafka brokers configured: irrelevant here, ownership must be
    // checked (and must fail) before any publish is attempted.
    let schema = create_schema(db.clone(), common::dummy_settings());
    let intruder = AuthenticatedUser {
        user_id: user_b,
        username: "intruder-mut-test".to_string(),
        account_ids: vec![account_b],
    };

    let tags_query = format!(r#"mutation {{ setTransactionTags(transactionId: "{tx}", tags: ["x"]) {{ id }} }}"#);
    let tags_response = schema.execute(authed(Request::new(tags_query), Some(intruder.clone()))).await;
    assert!(!tags_response.errors.is_empty(), "expected a cross-user denial");

    let recurring_query =
        format!(r#"mutation {{ setTransactionRecurring(transactionId: "{tx}", recurring: true) {{ id }} }}"#);
    let recurring_response = schema.execute(authed(Request::new(recurring_query), Some(intruder))).await;
    assert!(!recurring_response.errors.is_empty(), "expected a cross-user denial");
}

#[tokio::test]
async fn transactions_query_tag_filter_ands_multiple_tags() {
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "tag-filter-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Tag filter account").await;
    common::link(&db, user, account).await;

    let tx_both = common::seed_transaction(&db, account, "2024-07-08", "-1.00", Some("Shop")).await;
    seed_tag(&db, tx_both, "a").await;
    seed_tag(&db, tx_both, "b").await;
    let tx_only_a = common::seed_transaction(&db, account, "2024-07-08", "-2.00", Some("Shop")).await;
    seed_tag(&db, tx_only_a, "a").await;

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "tag-filter-test".to_string(),
        account_ids: vec![account],
    };

    let query = r#"{ transactions(filter: { tags: ["a", "b"] }) { items { id } } }"#;
    let response = schema.execute(authed(Request::new(query), Some(caller))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let found_ids: Vec<String> = data["transactions"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(found_ids, vec![tx_both.to_string()]);
}

#[tokio::test]
async fn recurring_series_reports_full_series_extent_under_a_narrow_date_filter() {
    let db = common::db().await;

    let (user, _) = common::seed_user(&db, "series-extent-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Series extent account").await;
    common::link(&db, user, account).await;

    let series_id = Uuid::new_v4();
    let tx_jan = common::seed_transaction(&db, account, "2024-01-15", "-10.00", Some("Gym")).await;
    let tx_feb = common::seed_transaction(&db, account, "2024-02-15", "-10.00", Some("Gym")).await;
    let tx_mar = common::seed_transaction(&db, account, "2024-03-15", "-10.00", Some("Gym")).await;
    for tx in [tx_jan, tx_feb, tx_mar] {
        seed_insight(&db, tx, false, None, true, Some(series_id), Some("monthly"), Some("-10.00")).await;
    }

    let schema = create_schema(db.clone(), common::dummy_settings());
    let caller = AuthenticatedUser {
        user_id: user,
        username: "series-extent-test".to_string(),
        account_ids: vec![account],
    };

    // Filter narrows to just February; the returned series must still
    // report the full Jan-Mar extent (3 occurrences), not just February's.
    let query = r#"{ recurringSeries(filter: { startDate: "2024-02-01", endDate: "2024-02-29" }) {
        series { id occurrenceCount firstDate lastDate }
        totalMonthlyEquivalent
    } }"#;
    let response = schema.execute(authed(Request::new(query), Some(caller))).await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let data = response.data.into_json().unwrap();
    let series = data["recurringSeries"]["series"].as_array().unwrap();
    assert_eq!(series.len(), 1, "expected exactly one series: {series:?}");
    assert_eq!(series[0]["occurrenceCount"], 3);
    assert_eq!(series[0]["firstDate"], "2024-01-15");
    assert_eq!(series[0]["lastDate"], "2024-03-15");
}
