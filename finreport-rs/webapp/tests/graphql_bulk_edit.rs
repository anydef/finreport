//! Integration tests for `setTransactionsCategory` / `setTransactionsTags`
//! and the `transactionIds` / `amountMin` / `amountMax` / `counterpartyKeys`
//! filter fields, plus the review flow's idempotent `ensureCategory`.
//! Gated behind the `integration` feature like the rest of the suite; a
//! throwaway Kafka container backs the publish side, Postgres is the shared
//! `finreport-wp4-pg` instance (`common::db`).
#![cfg(feature = "integration")]

mod common;

use async_graphql::Request;
use chrono::Utc;
use entity::entities::{transaction_split, transaction_tag, transaction_user_label};
use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::{ClientConfig, Message, Offset, TopicPartitionList};
use rust_decimal::Decimal;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, RawSessionToken};
use webapp::kafka::labeling::{TOPIC_CATEGORY, TOPIC_LABEL_REQUEST, TOPIC_RULE, TOPIC_USER_LABEL};

type KafkaContainer = testcontainers::ContainerAsync<testcontainers_modules::kafka::apache::Kafka>;

fn authed(request: Request, user: &AuthenticatedUser) -> Request {
    request_with_auth(request, Some(user.clone()), RawSessionToken::default())
}

async fn start_kafka() -> (KafkaContainer, String) {
    use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
    use rdkafka::client::DefaultClientContext;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::kafka::apache::{Kafka, KAFKA_PORT};

    let container = Kafka::default().start().await.expect("start Kafka testcontainer");
    let host = container.get_host().await.expect("Kafka host");
    let port = container.get_host_port_ipv4(KAFKA_PORT).await.expect("Kafka port");
    let bootstrap = format!("{host}:{port}");
    let admin: AdminClient<DefaultClientContext> = ClientConfig::new()
        .set("bootstrap.servers", &bootstrap)
        .create()
        .expect("admin client");
    let topics = [TOPIC_USER_LABEL, TOPIC_RULE, TOPIC_CATEGORY, TOPIC_LABEL_REQUEST]
        .map(|name| NewTopic::new(name, 1, TopicReplication::Fixed(1)));
    admin
        .create_topics(&topics, &AdminOptions::new().request_timeout(Some(Duration::from_secs(10))))
        .await
        .expect("create topics");
    (container, bootstrap)
}

fn settings(bootstrap: &str, bulk_max: u32) -> Arc<utils::settings::Settings> {
    let mut s = (*common::dummy_settings()).clone();
    s.kafka_brokers = Some(bootstrap.to_string());
    s.bulk_edit_max_transactions = bulk_max;
    Arc::new(s)
}

/// Every `finreport.user-label` record on the topic, as parsed JSON, in
/// publish order.
fn read_user_labels(bootstrap: &str) -> Vec<serde_json::Value> {
    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", bootstrap)
        .set("group.id", format!("bulk-test-{}", Uuid::new_v4()))
        .create()
        .expect("consumer");
    let mut tpl = TopicPartitionList::new();
    tpl.add_partition_offset(TOPIC_USER_LABEL, 0, Offset::Beginning).unwrap();
    consumer.assign(&tpl).unwrap();
    let mut out = Vec::new();
    let mut quiet_since = Instant::now();
    while quiet_since.elapsed() < Duration::from_secs(2) {
        if let Some(Ok(msg)) = consumer.poll(Duration::from_millis(200)) {
            out.push(serde_json::from_slice(msg.payload().unwrap()).unwrap());
            quiet_since = Instant::now();
        }
    }
    out
}

struct World {
    db: Arc<DatabaseConnection>,
    user: AuthenticatedUser,
    account: Uuid,
    schema: webapp::graphql::AppSchema,
    bootstrap: String,
    _kafka: KafkaContainer,
}

async fn world_with_limit(bulk_max: u32) -> World {
    let (kafka, bootstrap) = start_kafka().await;
    let db = common::db().await;
    let (user_id, username) = common::seed_user(&db, "bulk-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Bulk account").await;
    common::link(&db, user_id, account).await;
    let schema = create_schema(db.clone(), settings(&bootstrap, bulk_max));
    World {
        db,
        user: AuthenticatedUser { user_id, username, account_ids: vec![account] },
        account,
        schema,
        bootstrap,
        _kafka: kafka,
    }
}

async fn world() -> World {
    world_with_limit(5_000).await
}

impl World {
    async fn tx(&self, amount: &str) -> Uuid {
        common::seed_transaction(&self.db, self.account, "2024-07-01", amount, Some("Shop")).await
    }

    async fn run(&self, query: String) -> async_graphql::Response {
        self.schema.execute(authed(Request::new(query), &self.user)).await
    }

    async fn run_ok(&self, query: String) -> serde_json::Value {
        let response = self.run(query).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        response.data.into_json().unwrap()
    }

    async fn user_label(&self, tx: Uuid) -> Option<transaction_user_label::Model> {
        transaction_user_label::Entity::find_by_id(tx).one(self.db.as_ref()).await.unwrap()
    }

    async fn tags(&self, tx: Uuid) -> Vec<String> {
        let mut tags: Vec<String> = transaction_tag::Entity::find()
            .filter(transaction_tag::Column::TransactionId.eq(tx))
            .all(self.db.as_ref())
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.tag)
            .collect();
        tags.sort();
        tags
    }

    async fn seed_tag(&self, tx: Uuid, tag: &str) {
        transaction_tag::ActiveModel {
            transaction_id: Set(tx),
            tag: Set(tag.to_string()),
            revision: Set(Utc::now().into()),
        }
        .insert(self.db.as_ref())
        .await
        .unwrap();
    }

    async fn seed_split(&self, tx: Uuid, index: i32, amount: &str, category: Uuid) {
        transaction_split::ActiveModel {
            id: Set(Uuid::new_v4()),
            transaction_id: Set(tx),
            part_index: Set(index),
            amount: Set(Decimal::from_str(amount).unwrap()),
            category_id: Set(category),
            invalid: Set(false),
        }
        .insert(self.db.as_ref())
        .await
        .unwrap();
    }

    async fn split_count(&self, tx: Uuid) -> usize {
        transaction_split::Entity::find()
            .filter(transaction_split::Column::TransactionId.eq(tx))
            .all(self.db.as_ref())
            .await
            .unwrap()
            .len()
    }
}

fn id_list(ids: &[Uuid]) -> String {
    ids.iter().map(|i| format!("\"{i}\"")).collect::<Vec<_>>().join(", ")
}

fn bulk_category(ids: &[Uuid], slug: &str) -> String {
    format!(
        r#"mutation {{ setTransactionsCategory(filter: {{ transactionIds: [{}] }}, categorySlug: "{slug}") {{
            matched applied failed splitsCleared }} }}"#,
        id_list(ids)
    )
}

fn bulk_tags(ids: &[Uuid], tags: &str) -> String {
    format!(
        r#"mutation {{ setTransactionsTags(filter: {{ transactionIds: [{}] }}, tags: [{tags}]) {{
            matched applied failed splitsCleared }} }}"#,
        id_list(ids)
    )
}

#[tokio::test]
async fn bulk_category_preserves_tags_clears_and_counts_splits() {
    let w = world().await;
    let (_, slug) = common::seed_category(&w.db, "expense.bulk-cat", "Bulk", "EXPENSE").await;
    let (other_cat, _) = common::seed_category(&w.db, "expense.bulk-old", "Old", "EXPENSE").await;
    let tagged = w.tx("-10.00").await;
    let split = w.tx("-20.00").await;
    let plain = w.tx("-30.00").await;
    w.seed_tag(tagged, "keepme").await;
    w.seed_split(split, 0, "-5.00", other_cat).await;
    w.seed_split(split, 1, "-15.00", other_cat).await;

    let data = w.run_ok(bulk_category(&[tagged, split, plain], &slug)).await;
    let result = &data["setTransactionsCategory"];
    assert_eq!(result["matched"], 3);
    assert_eq!(result["applied"], 3);
    assert_eq!(result["failed"], 0);
    assert_eq!(result["splitsCleared"], 2, "the two split rows of one transaction");

    let category_id = entity::entities::category::Entity::find()
        .filter(entity::entities::category::Column::Slug.eq(slug.as_str()))
        .one(w.db.as_ref())
        .await
        .unwrap()
        .unwrap()
        .id;
    for tx in [tagged, split, plain] {
        assert_eq!(w.user_label(tx).await.unwrap().category_id, Some(category_id));
    }
    assert_eq!(w.split_count(split).await, 0);
    assert_eq!(w.tags(tagged).await, vec!["keepme"]);

    // The published whole-state record must carry the tag too, or a replay
    // of the log would silently drop it.
    let tagged_external = tagged.to_string();
    let records = read_user_labels(&w.bootstrap);
    let record = records
        .iter()
        .rev()
        .find(|r| r["external_id"] == tagged_external.as_str())
        .expect("published user-label record for the tagged transaction");
    assert_eq!(record["category_slug"], slug.as_str());
    assert_eq!(record["tags"], serde_json::json!(["keepme"]));
}

#[tokio::test]
async fn bulk_tags_preserve_each_rows_category() {
    let w = world().await;
    let (cat_a, slug_a) = common::seed_category(&w.db, "expense.bulk-a", "A", "EXPENSE").await;
    let (cat_b, slug_b) = common::seed_category(&w.db, "expense.bulk-b", "B", "EXPENSE").await;
    let one = w.tx("-10.00").await;
    let two = w.tx("-11.00").await;
    let bare = w.tx("-12.00").await;
    w.run_ok(bulk_category(&[one], &slug_a)).await;
    w.run_ok(bulk_category(&[two], &slug_b)).await;
    w.seed_tag(one, "old").await;

    let data = w.run_ok(bulk_tags(&[one, two, bare], r#""Travel", "travel", " Food ""#)).await;
    let result = &data["setTransactionsTags"];
    assert_eq!((result["matched"].as_i64(), result["applied"].as_i64()), (Some(3), Some(3)));
    assert_eq!(result["splitsCleared"], 0);

    assert_eq!(w.user_label(one).await.unwrap().category_id, Some(cat_a));
    assert_eq!(w.user_label(two).await.unwrap().category_id, Some(cat_b));
    assert_eq!(w.user_label(bare).await.unwrap().category_id, None);
    for tx in [one, two, bare] {
        assert_eq!(w.tags(tx).await, vec!["food", "travel"], "override, normalised, `old` gone");
    }
}

#[tokio::test]
async fn another_users_transaction_in_the_filter_is_silently_ignored() {
    let w = world().await;
    let (_, slug) = common::seed_category(&w.db, "expense.bulk-x", "X", "EXPENSE").await;
    let mine = w.tx("-10.00").await;

    let (_other_user, _) = common::seed_user(&w.db, "bulk-other", "pw").await;
    let other_account = common::seed_account(&w.db, "EUR", "Other").await;
    let theirs =
        common::seed_transaction(&w.db, other_account, "2024-07-01", "-10.00", Some("Shop")).await;

    let data = w.run_ok(bulk_category(&[mine, theirs], &slug)).await;
    assert_eq!(data["setTransactionsCategory"]["matched"], 1);
    assert!(w.user_label(theirs).await.is_none(), "must not touch another user's row");

    let data = w.run_ok(bulk_tags(&[theirs], r#""sneaky""#)).await;
    assert_eq!(data["setTransactionsTags"]["matched"], 0);
    assert!(w.tags(theirs).await.is_empty());
}

#[tokio::test]
async fn one_failing_row_is_counted_and_the_others_are_applied() {
    let w = world().await;
    let (_, slug) = common::seed_category(&w.db, "expense.bulk-f", "F", "EXPENSE").await;
    let ok_a = w.tx("-1.00").await;
    let bad = w.tx("-2.00").await;
    let ok_b = w.tx("-3.00").await;

    // Make the projection upsert for exactly one transaction blow up.
    let suffix = bad.simple().to_string();
    w.db.execute_unprepared(&format!(
        "CREATE FUNCTION bulk_fail_{suffix}() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'injected'; END; $$ LANGUAGE plpgsql;
         CREATE TRIGGER bulk_fail_{suffix} BEFORE INSERT OR UPDATE ON transaction_user_label
           FOR EACH ROW WHEN (NEW.transaction_id = '{bad}') EXECUTE FUNCTION bulk_fail_{suffix}();"
    ))
    .await
    .unwrap();

    let data = w.run_ok(bulk_category(&[ok_a, bad, ok_b], &slug)).await;
    let result = &data["setTransactionsCategory"];
    assert_eq!(result["matched"], 3);
    assert_eq!(result["applied"], 2);
    assert_eq!(result["failed"], 1);
    assert!(w.user_label(ok_a).await.is_some());
    assert!(w.user_label(ok_b).await.is_some());
    assert!(w.user_label(bad).await.is_none());

    // Retrying once the fault is gone finishes the job, and only that job.
    w.db.execute_unprepared(&format!(
        "DROP TRIGGER bulk_fail_{suffix} ON transaction_user_label; DROP FUNCTION bulk_fail_{suffix}();"
    ))
    .await
    .unwrap();
    let data = w.run_ok(bulk_category(&[ok_a, bad, ok_b], &slug)).await;
    let result = &data["setTransactionsCategory"];
    assert_eq!((result["applied"].as_i64(), result["failed"].as_i64()), (Some(1), Some(0)));
    assert!(w.user_label(bad).await.is_some());
}

#[tokio::test]
async fn repeating_a_bulk_edit_is_idempotent() {
    let w = world().await;
    let (_, slug) = common::seed_category(&w.db, "expense.bulk-r", "R", "EXPENSE").await;
    let ids = [w.tx("-1.00").await, w.tx("-2.00").await];

    let first = w.run_ok(bulk_category(&ids, &slug)).await;
    assert_eq!(first["setTransactionsCategory"]["applied"], 2);
    let second = w.run_ok(bulk_category(&ids, &slug)).await;
    assert_eq!(second["setTransactionsCategory"]["matched"], 2);
    assert_eq!(second["setTransactionsCategory"]["applied"], 0);
    assert_eq!(second["setTransactionsCategory"]["failed"], 0);

    let first = w.run_ok(bulk_tags(&ids, r#""x""#)).await;
    assert_eq!(first["setTransactionsTags"]["applied"], 2);
    let second = w.run_ok(bulk_tags(&ids, r#""x""#)).await;
    assert_eq!(second["setTransactionsTags"]["applied"], 0);
    for tx in ids {
        assert_eq!(w.tags(tx).await, vec!["x"]);
    }
}

#[tokio::test]
async fn amount_bounds_apply_to_the_magnitude_in_both_directions() {
    let w = world().await;
    let spend_big = w.tx("-150.00").await;
    let income_big = w.tx("150.00").await;
    let spend_small = w.tx("-50.00").await;
    let income_small = w.tx("50.00").await;
    let all = [spend_big, income_big, spend_small, income_small];

    let count = |filter: String| {
        let w = &w;
        async move {
            let data = w
                .run_ok(format!(
                    "{{ transactions(filter: {{ transactionIds: [{}] {filter} }}) {{ items {{ id }} }} }}",
                    id_list(&all)
                ))
                .await;
            let mut ids: Vec<String> = data["transactions"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["id"].as_str().unwrap().to_string())
                .collect();
            ids.sort();
            ids
        }
    };
    let sorted = |ids: &[Uuid]| {
        let mut v: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        v.sort();
        v
    };

    assert_eq!(count(r#"amountMin: "100""#.into()).await, sorted(&[spend_big, income_big]));
    assert_eq!(count(r#"amountMax: "100""#.into()).await, sorted(&[spend_small, income_small]));
    assert_eq!(
        count(r#"amountMin: "50" amountMax: "50""#.into()).await,
        sorted(&[spend_small, income_small]),
        "bounds are inclusive"
    );
    assert_eq!(
        count(r#"amountMin: "100" direction: SPENDING"#.into()).await,
        sorted(&[spend_big])
    );
    assert!(count(r#"amountMin: "200""#.into()).await.is_empty());

    // The aggregations honour the same fields, or "all matching" would mean
    // different rows in different places.
    let summary = w
        .run_ok(format!(
            r#"{{ cashflowSummary(filter: {{ transactionIds: [{}] startDate: "2024-07-01" endDate: "2024-07-31" amountMin: "100" }}, granularity: MONTH) {{
                buckets {{ income spending transactionCount }} }} }}"#,
            id_list(&all)
        ))
        .await;
    let bucket = &summary["cashflowSummary"]["buckets"][0];
    assert_eq!(bucket["transactionCount"], 2);
    assert_eq!(Decimal::from_str(bucket["income"].as_str().unwrap()).unwrap(), Decimal::from(150));
    assert_eq!(Decimal::from_str(bucket["spending"].as_str().unwrap()).unwrap(), Decimal::from(150));

    let graph = w
        .run_ok(format!(
            r#"{{ cashflowGraph(filter: {{ transactionIds: [{}] startDate: "2024-07-01" endDate: "2024-07-31" amountMax: "10" }}) {{
                links {{ value }} }} }}"#,
            id_list(&all)
        ))
        .await;
    assert!(graph["cashflowGraph"]["links"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn bulk_edit_can_be_driven_by_amount_bounds_alone() {
    let w = world().await;
    let (_, slug) = common::seed_category(&w.db, "expense.bulk-amt", "Amt", "EXPENSE").await;
    let big_out = w.tx("-150.00").await;
    let big_in = w.tx("150.00").await;
    let small = w.tx("-5.00").await;

    let data = w
        .run_ok(format!(
            r#"mutation {{ setTransactionsCategory(filter: {{ amountMin: "100" }}, categorySlug: "{slug}") {{ matched applied }} }}"#
        ))
        .await;
    assert_eq!(data["setTransactionsCategory"]["matched"], 2);
    assert!(w.user_label(big_out).await.is_some());
    assert!(w.user_label(big_in).await.is_some());
    assert!(w.user_label(small).await.is_none());
}

#[tokio::test]
async fn an_empty_filter_means_every_transaction_the_caller_owns_and_nobody_elses() {
    let w = world().await;
    let (_, slug) = common::seed_category(&w.db, "expense.bulk-all", "All", "EXPENSE").await;
    let a = w.tx("-1.00").await;
    let b = w.tx("-2.00").await;
    let other_account = common::seed_account(&w.db, "EUR", "Stranger").await;
    let theirs = common::seed_transaction(&w.db, other_account, "2024-07-01", "-3.00", None).await;

    let data = w
        .run_ok(format!(
            r#"mutation {{ setTransactionsCategory(filter: {{}}, categorySlug: "{slug}") {{ matched applied }} }}"#
        ))
        .await;
    assert_eq!(data["setTransactionsCategory"]["matched"], 2);
    assert!(w.user_label(a).await.is_some() && w.user_label(b).await.is_some());
    assert!(w.user_label(theirs).await.is_none());
}

#[tokio::test]
async fn validation_failures_change_nothing() {
    let w = world_with_limit(2).await;
    let (_, slug) = common::seed_category(&w.db, "expense.bulk-v", "V", "EXPENSE").await;
    let ids = [w.tx("-1.00").await, w.tx("-2.00").await, w.tx("-3.00").await];

    let unknown = w.run(bulk_category(&ids, "no-such-category")).await;
    assert_eq!(unknown.errors[0].extensions.as_ref().unwrap().get("code"), Some(&"VALIDATION".into()));

    let too_many_tags = w
        .run(bulk_tags(
            &ids[..2],
            &(0..11).map(|i| format!("\"t{i}\"")).collect::<Vec<_>>().join(", "),
        ))
        .await;
    assert_eq!(
        too_many_tags.errors[0].extensions.as_ref().unwrap().get("code"),
        Some(&"TOO_MANY_TAGS".into())
    );

    // Three matches against a cap of two: rejected outright, nothing applied.
    let capped = w.run(bulk_category(&ids, &slug)).await;
    assert_eq!(
        capped.errors[0].extensions.as_ref().unwrap().get("code"),
        Some(&"BULK_LIMIT_EXCEEDED".into())
    );
    for tx in ids {
        assert!(w.user_label(tx).await.is_none());
    }

    // At the cap exactly is fine.
    let data = w.run_ok(bulk_category(&ids[..2], &slug)).await;
    assert_eq!(data["setTransactionsCategory"]["applied"], 2);
}

impl World {
    async fn set_key(&self, tx: Uuid, key: &str) {
        self.db
            .execute_unprepared(&format!(
                "UPDATE transaction SET counterparty_key = '{key}' WHERE id = '{tx}'"
            ))
            .await
            .unwrap();
    }

    /// Ids of `transactions(filter: {...})`, sorted.
    async fn filtered_ids(&self, filter: &str) -> Vec<String> {
        let data = self
            .run_ok(format!("{{ transactions(filter: {{ {filter} }}) {{ items {{ id }} }} }}"))
            .await;
        let mut ids: Vec<String> = data["transactions"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids
    }
}

fn sorted_ids(ids: &[Uuid]) -> Vec<String> {
    let mut v: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
    v.sort();
    v
}

#[tokio::test]
async fn counterparty_keys_select_every_variant_sharing_a_key_and_nothing_else() {
    let w = world().await;
    let tag = Uuid::new_v4().simple().to_string();
    let (lidl, rewe) = (format!("lidl_{tag}"), format!("rewe_{tag}"));
    // Three raw spellings of one merchant, one other merchant, one unkeyed.
    let a = common::seed_transaction(&w.db, w.account, "2024-07-01", "-10.00", Some("LIDL SAGT DANKE")).await;
    let b = common::seed_transaction(&w.db, w.account, "2024-07-02", "-11.00", Some("Lidl Berlin 123")).await;
    let c = common::seed_transaction(&w.db, w.account, "2024-07-03", "-12.00", Some("LIDL DIGITAL")).await;
    let other = common::seed_transaction(&w.db, w.account, "2024-07-04", "-13.00", Some("REWE")).await;
    let unkeyed = common::seed_transaction(&w.db, w.account, "2024-07-05", "-14.00", Some("LIDL SAGT DANKE")).await;
    for tx in [a, b, c] {
        w.set_key(tx, &lidl).await;
    }
    w.set_key(other, &rewe).await;

    assert_eq!(
        w.filtered_ids(&format!(r#"counterpartyKeys: ["{lidl}"]"#)).await,
        sorted_ids(&[a, b, c]),
        "every variant sharing the key, and not the unkeyed or other-key rows"
    );
    assert_eq!(
        w.filtered_ids(&format!(r#"counterpartyKeys: ["{lidl}", "{rewe}"]"#)).await,
        sorted_ids(&[a, b, c, other]),
        "keys are OR-ed"
    );
    assert!(w.filtered_ids(r#"counterpartyKeys: ["no_such_key"]"#).await.is_empty());
    // The raw-name filter misses the variants: the reason this filter exists.
    assert_eq!(
        w.filtered_ids(r#"counterpartyNames: ["LIDL SAGT DANKE"]"#).await,
        sorted_ids(&[a, unkeyed])
    );
    // The row exposes its key so a client can filter by it.
    let data = w
        .run_ok(format!(
            r#"{{ transactions(filter: {{ transactionIds: ["{a}"] }}) {{ items {{ counterpartyKey }} }} }}"#
        ))
        .await;
    assert_eq!(data["transactions"]["items"][0]["counterpartyKey"], lidl.as_str());

    // The aggregations honour it too.
    let summary = w
        .run_ok(format!(
            r#"{{ cashflowSummary(filter: {{ counterpartyKeys: ["{lidl}"] startDate: "2024-07-01" endDate: "2024-07-31" }}, granularity: MONTH) {{
                buckets {{ spending transactionCount }} }} }}"#
        ))
        .await;
    let bucket = &summary["cashflowSummary"]["buckets"][0];
    assert_eq!(bucket["transactionCount"], 3);
    assert_eq!(Decimal::from_str(bucket["spending"].as_str().unwrap()).unwrap(), Decimal::from(33));
    // The graph over the key equals the graph over exactly those ids.
    let graph_total = |filter: String| {
        let w = &w;
        async move {
            let graph = w
                .run_ok(format!(
                    r#"{{ cashflowGraph(filter: {{ {filter} startDate: "2024-07-01" endDate: "2024-07-31" }}) {{ links {{ value }} }} }}"#
                ))
                .await;
            graph["cashflowGraph"]["links"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| Decimal::from_str(l["value"].as_str().unwrap()).unwrap())
                .sum::<Decimal>()
        }
    };
    let by_key = graph_total(format!(r#"counterpartyKeys: ["{lidl}"]"#)).await;
    let by_ids = graph_total(format!(r#"transactionIds: ["{a}", "{b}", "{c}"]"#)).await;
    assert!(by_key > Decimal::ZERO);
    assert_eq!(by_key, by_ids);
}

#[tokio::test]
async fn a_bulk_edit_can_be_driven_by_counterparty_keys() {
    let w = world().await;
    let (_, slug) = common::seed_category(&w.db, "expense.bulk-key", "Key", "EXPENSE").await;
    let key = format!("lidl_{}", Uuid::new_v4().simple());
    let a = common::seed_transaction(&w.db, w.account, "2024-07-01", "-10.00", Some("LIDL A")).await;
    let b = common::seed_transaction(&w.db, w.account, "2024-07-02", "-11.00", Some("Lidl B")).await;
    let other = common::seed_transaction(&w.db, w.account, "2024-07-03", "-12.00", Some("REWE")).await;
    w.set_key(a, &key).await;
    w.set_key(b, &key).await;
    w.set_key(other, "unrelated").await;

    let data = w
        .run_ok(format!(
            r#"mutation {{ setTransactionsCategory(filter: {{ counterpartyKeys: ["{key}"] }}, categorySlug: "{slug}") {{ matched applied }} }}"#
        ))
        .await;
    assert_eq!(data["setTransactionsCategory"]["matched"], 2);
    assert_eq!(data["setTransactionsCategory"]["applied"], 2);
    assert!(w.user_label(a).await.is_some());
    assert!(w.user_label(b).await.is_some());
    assert!(w.user_label(other).await.is_none());

    let data = w
        .run_ok(format!(
            r#"mutation {{ setTransactionsTags(filter: {{ counterpartyKeys: ["{key}"] }}, tags: ["lidl"]) {{ matched applied }} }}"#
        ))
        .await;
    assert_eq!(data["setTransactionsTags"]["matched"], 2);
    assert_eq!(w.tags(a).await, vec!["lidl"]);
    assert!(w.tags(other).await.is_empty());
}

fn ensure_category(slug: &str, kind: &str, parent: Option<&str>) -> String {
    let parent = parent.map(|p| format!(r#", parentSlug: "{p}""#)).unwrap_or_default();
    format!(
        r#"mutation {{ ensureCategory(input: {{ slug: "{slug}", name: "Proposed", kind: {kind}{parent} }}) {{ id slug kind }} }}"#
    )
}

fn error_code(response: &async_graphql::Response) -> Option<String> {
    response
        .errors
        .first()
        .and_then(|e| e.extensions.as_ref())
        .and_then(|x| x.get("code"))
        .map(|c| c.to_string().trim_matches('"').to_string())
}

#[tokio::test]
async fn approving_the_same_proposed_category_twice_assigns_both_to_one_category() {
    let w = world().await;
    let slug = format!("proposed_{}", Uuid::new_v4().simple());
    let first = w.tx("-10.00").await;
    let second = w.tx("-20.00").await;

    // What the review screen does per held transaction: ensure, then assign.
    let mut ids = Vec::new();
    for tx in [first, second] {
        let data = w.run_ok(ensure_category(&slug, "EXPENSE", None)).await;
        ids.push(data["ensureCategory"]["id"].as_str().unwrap().to_string());
        w.run_ok(format!(
            r#"mutation {{ setTransactionCategory(transactionId: "{tx}", categorySlug: "{slug}") {{ id }} }}"#
        ))
        .await;
    }
    assert_eq!(ids[0], ids[1], "the second call returns the category the first created");
    let cat = Uuid::parse_str(&ids[0]).unwrap();
    assert_eq!(w.user_label(first).await.unwrap().category_id, Some(cat));
    assert_eq!(w.user_label(second).await.unwrap().category_id, Some(cat));
}

#[tokio::test]
async fn create_category_stays_strict_about_duplicates() {
    let w = world().await;
    let slug = format!("strict_{}", Uuid::new_v4().simple());
    w.run_ok(ensure_category(&slug, "EXPENSE", None)).await;
    let dup = w
        .run(format!(
            r#"mutation {{ createCategory(input: {{ slug: "{slug}", name: "Again", kind: EXPENSE }}) {{ id }} }}"#
        ))
        .await;
    assert!(!dup.errors.is_empty(), "createCategory must still reject a duplicate");
    assert!(dup.errors[0].message.contains("already exists"), "{:?}", dup.errors);
}

#[tokio::test]
async fn ensure_category_rejects_a_conflicting_definition() {
    let w = world().await;
    let parent = format!("parent_{}", Uuid::new_v4().simple());
    let other_parent = format!("otherparent_{}", Uuid::new_v4().simple());
    let child = format!("{parent}.child");
    w.run_ok(ensure_category(&parent, "EXPENSE", None)).await;
    w.run_ok(ensure_category(&other_parent, "EXPENSE", None)).await;
    w.run_ok(ensure_category(&child, "EXPENSE", Some(&parent))).await;

    // Same definition: fine (and the name is not part of the identity).
    w.run_ok(ensure_category(&child, "EXPENSE", Some(&parent))).await;

    // Different kind, then different parent: CONFLICT, nothing changed.
    let kind = w.run(ensure_category(&child, "INCOME", Some(&parent))).await;
    assert_eq!(error_code(&kind).as_deref(), Some("CONFLICT"), "{:?}", kind.errors);
    let parented = w.run(ensure_category(&child, "EXPENSE", Some(&other_parent))).await;
    assert_eq!(error_code(&parented).as_deref(), Some("CONFLICT"), "{:?}", parented.errors);
    let orphan = w.run(ensure_category(&child, "EXPENSE", None)).await;
    assert_eq!(error_code(&orphan).as_deref(), Some("CONFLICT"), "{:?}", orphan.errors);
}
