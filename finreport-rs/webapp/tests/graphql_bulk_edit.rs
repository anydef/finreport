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
use webapp::kafka::links::TOPIC_TRANSACTION_LINK;

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
    let topics = [TOPIC_USER_LABEL, TOPIC_RULE, TOPIC_CATEGORY, TOPIC_LABEL_REQUEST, TOPIC_TRANSACTION_LINK]
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

#[tokio::test]
async fn child_category_slug_must_be_parent_slug_plus_one_segment() {
    let w = world().await;
    let parent = format!("par_{}", Uuid::new_v4().simple());
    w.run_ok(ensure_category(&parent, "EXPENSE", None)).await;
    let create = |slug: String, parent: Option<String>| {
        let p = parent.map(|p| format!(r#", parentSlug: "{p}""#)).unwrap_or_default();
        format!(
            r#"mutation {{ createCategory(input: {{ slug: "{slug}", name: "X", kind: EXPENSE{p} }}) {{ id slug }} }}"#
        )
    };

    // Parent + one segment: accepted, and findable as a descendant by prefix.
    let ok = w.run_ok(create(format!("{parent}.gym"), Some(parent.clone()))).await;
    assert_eq!(ok["createCategory"]["slug"], format!("{parent}.gym"));

    // A bare leaf: rejected with the slug it should have been.
    let bare = w.run(create("gym".into(), Some(parent.clone()))).await;
    assert_eq!(error_code(&bare).as_deref(), Some("VALIDATION"), "{:?}", bare.errors);
    assert!(
        bare.errors[0].message.contains(&format!("e.g. '{parent}.gym'")),
        "{:?}",
        bare.errors
    );

    // Two segments below the parent: rejected.
    let deep = w.run(create(format!("{parent}.a.b"), Some(parent.clone()))).await;
    assert_eq!(error_code(&deep).as_deref(), Some("VALIDATION"), "{:?}", deep.errors);

    // A dotted slug with no parent: rejected, naming the parent to pass.
    let orphan = w.run(create(format!("{parent}.other"), None)).await;
    assert_eq!(error_code(&orphan).as_deref(), Some("VALIDATION"), "{:?}", orphan.errors);
    assert!(orphan.errors[0].message.contains("parentSlug"), "{:?}", orphan.errors);

    // ensureCategory enforces the same rule when it has to create.
    let ensured = w.run(ensure_category("gym2", "EXPENSE", Some(&parent))).await;
    assert_eq!(error_code(&ensured).as_deref(), Some("VALIDATION"), "{:?}", ensured.errors);
}

// ---------------------------------------------------------------------------
// setTransactionNote: free-text commentary riding in the whole-state record
// ---------------------------------------------------------------------------

impl World {
    async fn note(&self, tx: Uuid) -> Option<String> {
        self.user_label(tx).await.and_then(|l| l.note)
    }

    async fn set_note(&self, tx: Uuid, note: &str) -> async_graphql::Response {
        let arg = if note == "null" { "null".to_string() } else { format!("{note:?}") };
        self.run(format!(
            r#"mutation {{ setTransactionNote(transactionId: "{tx}", note: {arg}) {{ id note }} }}"#
        ))
        .await
    }

    /// The last published whole-state record for `tx`.
    fn last_record(&self, tx: Uuid) -> serde_json::Value {
        let external = tx.to_string();
        read_user_labels(&self.bootstrap)
            .into_iter()
            .rev()
            .find(|r| r["external_id"] == external.as_str())
            .expect("a published user-label record")
    }
}

#[tokio::test]
async fn setting_a_note_round_trips_and_preserves_category_tags_recurring_and_splits() {
    let w = world().await;
    let (cat_id, slug) = common::seed_category(&w.db, "expense.note-cat", "Note", "EXPENSE").await;
    let (_, slug_b) = common::seed_category(&w.db, "expense.note-b", "NoteB", "EXPENSE").await;
    let plain = w.tx("-20.00").await;
    let split = w.tx("-30.00").await;

    w.run_ok(format!(r#"mutation {{ setTransactionCategory(transactionId: "{plain}", categorySlug: "{slug}") {{ id }} }}"#)).await;
    w.run_ok(format!(r#"mutation {{ setTransactionTags(transactionId: "{plain}", tags: ["trip"]) {{ id }} }}"#)).await;
    w.run_ok(format!(r#"mutation {{ setTransactionRecurring(transactionId: "{plain}", recurring: true) {{ id }} }}"#)).await;
    w.run_ok(format!(
        r#"mutation {{ splitTransaction(transactionId: "{split}", parts: [{{amount: "-10.00", categorySlug: "{slug}"}}, {{amount: "-20.00", categorySlug: "{slug_b}"}}]) {{ id }} }}"#
    ))
    .await;

    let data = w.set_note(plain, "  Paid cash, ask Anna for half \n").await;
    assert!(data.errors.is_empty(), "{:?}", data.errors);
    assert_eq!(
        data.data.into_json().unwrap()["setTransactionNote"]["note"],
        "Paid cash, ask Anna for half",
        "trimmed, and returned by the mutation"
    );
    w.run_ok(format!(r#"mutation {{ setTransactionNote(transactionId: "{split}", note: "shared") {{ id }} }}"#)).await;

    // The projection row: note set, everything else untouched.
    let label = w.user_label(plain).await.unwrap();
    assert_eq!(label.note.as_deref(), Some("Paid cash, ask Anna for half"));
    assert_eq!(label.category_id, Some(cat_id));
    assert_eq!(label.recurring, Some(true));
    assert_eq!(w.tags(plain).await, vec!["trip"]);
    assert_eq!(w.split_count(split).await, 2);

    // The published whole-state record must carry all of it, or a replay of
    // the log would erase them.
    let record = w.last_record(plain);
    assert_eq!(record["note"], "Paid cash, ask Anna for half");
    assert_eq!(record["category_slug"], slug.as_str());
    assert_eq!(record["tags"], serde_json::json!(["trip"]));
    assert_eq!(record["recurring"], true);
    let record = w.last_record(split);
    assert_eq!(record["note"], "shared");
    assert_eq!(record["parts"].as_array().unwrap().len(), 2, "{record}");

    // Readable on the list query.
    let data = w
        .run_ok(format!(r#"{{ transactions(filter: {{ transactionIds: ["{plain}"] }}) {{ items {{ note }} }} }}"#))
        .await;
    assert_eq!(data["transactions"]["items"][0]["note"], "Paid cash, ask Anna for half");
    let untouched = w.tx("-1.00").await;
    let data = w
        .run_ok(format!(r#"{{ transactions(filter: {{ transactionIds: ["{untouched}"] }}) {{ items {{ note }} }} }}"#))
        .await;
    assert!(data["transactions"]["items"][0]["note"].is_null());
}

#[tokio::test]
async fn clearing_a_note_keeps_the_rest_and_bad_notes_are_rejected() {
    let w = world().await;
    let (cat_id, slug) = common::seed_category(&w.db, "expense.note-clear", "Clear", "EXPENSE").await;
    let tx = w.tx("-20.00").await;
    w.run_ok(format!(r#"mutation {{ setTransactionCategory(transactionId: "{tx}", categorySlug: "{slug}") {{ id }} }}"#)).await;

    assert!(w.set_note(tx, "remember").await.errors.is_empty());
    assert_eq!(w.note(tx).await.as_deref(), Some("remember"));

    // `null` clears.
    assert!(w.set_note(tx, "null").await.errors.is_empty());
    assert_eq!(w.note(tx).await, None);
    assert_eq!(w.user_label(tx).await.unwrap().category_id, Some(cat_id));
    let record = w.last_record(tx);
    assert!(record["note"].is_null(), "{record}");
    assert_eq!(record["category_slug"], slug.as_str());

    // Blank is "no note" too, not an empty string.
    assert!(w.set_note(tx, "x").await.errors.is_empty());
    assert!(w.set_note(tx, "   ").await.errors.is_empty());
    assert_eq!(w.note(tx).await, None);

    // Over-long is refused outright and changes nothing.
    assert!(w.set_note(tx, "keep").await.errors.is_empty());
    let too_long = w.set_note(tx, &"x".repeat(2001)).await;
    assert_eq!(error_code(&too_long).as_deref(), Some("VALIDATION"), "{:?}", too_long.errors);
    assert_eq!(w.note(tx).await.as_deref(), Some("keep"));
}

#[tokio::test]
async fn a_note_on_a_transaction_without_a_label_row_creates_a_commentary_only_override() {
    let w = world().await;
    let tx = w.tx("-5.00").await;
    assert!(w.user_label(tx).await.is_none());
    let r = w.set_note(tx, "just a thought").await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let label = w.user_label(tx).await.unwrap();
    assert_eq!(label.note.as_deref(), Some("just a thought"));
    assert_eq!(label.category_id, None, "a note must never pin a category");
    assert_eq!(label.recurring, None);
}

#[tokio::test]
async fn every_other_user_label_mutation_carries_the_note_through() {
    let w = world().await;
    let (_, slug) = common::seed_category(&w.db, "expense.note-keep", "Keep", "EXPENSE").await;
    let (_, slug_b) = common::seed_category(&w.db, "expense.note-keep-b", "KeepB", "EXPENSE").await;
    let tx = w.tx("-30.00").await;
    assert!(w.set_note(tx, "do not lose me").await.errors.is_empty());

    let mutations = [
        ("setTransactionCategory", format!(r#"setTransactionCategory(transactionId: "{tx}", categorySlug: "{slug}") {{ id }}"#)),
        ("setTransactionTags", format!(r#"setTransactionTags(transactionId: "{tx}", tags: ["a"]) {{ id }}"#)),
        ("setTransactionRecurring", format!(r#"setTransactionRecurring(transactionId: "{tx}", recurring: false) {{ id }}"#)),
        (
            "splitTransaction",
            format!(
                r#"splitTransaction(transactionId: "{tx}", parts: [{{amount: "-10.00", categorySlug: "{slug}"}}, {{amount: "-20.00", categorySlug: "{slug_b}"}}]) {{ id }}"#
            ),
        ),
        ("unsplitTransaction", format!(r#"unsplitTransaction(transactionId: "{tx}") {{ id }}"#)),
        ("setTransactionCategory again", format!(r#"setTransactionCategory(transactionId: "{tx}", categorySlug: "{slug_b}") {{ id }}"#)),
        ("clearTransactionCategory", format!(r#"clearTransactionCategory(transactionId: "{tx}") {{ id }}"#)),
    ];
    for (name, body) in mutations {
        w.run_ok(format!("mutation {{ {body} }}")).await;
        assert_eq!(w.note(tx).await.as_deref(), Some("do not lose me"), "{name} dropped the note");
    }

    // The bulk paths share the cores, but prove it end to end.
    w.run_ok(bulk_category(&[tx], &slug)).await;
    assert_eq!(w.note(tx).await.as_deref(), Some("do not lose me"), "bulk category dropped the note");
    w.run_ok(bulk_tags(&[tx], r#""b""#)).await;
    assert_eq!(w.note(tx).await.as_deref(), Some("do not lose me"), "bulk tags dropped the note");

    // And on the log, not just the projection: every record after the note
    // was first written still carries it.
    let external = tx.to_string();
    let records: Vec<_> = read_user_labels(&w.bootstrap)
        .into_iter()
        .filter(|r| r["external_id"] == external.as_str())
        .collect();
    assert!(records.len() >= 10, "{} records", records.len());
    for record in &records {
        assert_eq!(record["note"], "do not lose me", "{record}");
    }
}

// ---------------------------------------------------------------------------
// categorySlugsExact
// ---------------------------------------------------------------------------

impl World {
    async fn seed_child_category(&self, parent: Uuid, parent_slug: &str, leaf: &str) -> (Uuid, String) {
        let id = Uuid::new_v4();
        let slug = format!("{parent_slug}.{leaf}");
        entity::entities::category::ActiveModel {
            id: Set(id),
            slug: Set(slug.clone()),
            parent_id: Set(Some(parent)),
            name: Set(leaf.to_string()),
            kind: Set("EXPENSE".to_string()),
            depth: Set(2),
            sort_order: Set(0),
            archived: Set(false),
            origin: Set("seed".to_string()),
            owner_user_id: Set(None),
            revision: Set(Utc::now().into()),
        }
        .insert(self.db.as_ref())
        .await
        .unwrap();
        (id, slug)
    }
}

#[tokio::test]
async fn category_slugs_exact_selects_the_category_itself_and_not_its_children() {
    let w = world().await;
    let (parent, parent_slug) = common::seed_category(&w.db, "expense.exact", "Parent", "EXPENSE").await;
    let (child, child_slug) = w.seed_child_category(parent, &parent_slug, "kid").await;
    let (_, other_slug) = common::seed_category(&w.db, "expense.exact-other", "Other", "EXPENSE").await;

    let own = w.tx("-10.00").await;
    let own_big = w.tx("-300.00").await;
    let in_child = w.tx("-11.00").await;
    let split_into_parent = w.tx("-40.00").await;
    let split_only_child = w.tx("-50.00").await;
    let unlabelled = w.tx("-12.00").await;
    common::seed_transaction_label(&w.db, own, Some(parent), "user", "resolved").await;
    common::seed_transaction_label(&w.db, own_big, Some(parent), "llm", "resolved").await;
    common::seed_transaction_label(&w.db, in_child, Some(child), "user", "resolved").await;
    w.seed_split(split_into_parent, 0, "-15.00", parent).await;
    w.seed_split(split_into_parent, 1, "-25.00", child).await;
    w.seed_split(split_only_child, 0, "-50.00", child).await;
    let _ = unlabelled;

    // Descendant-expanding filter, for contrast: everything under the parent.
    assert_eq!(
        w.filtered_ids(&format!(r#"categorySlugs: ["{parent_slug}"]"#)).await,
        sorted_ids(&[own, own_big, in_child, split_into_parent, split_only_child])
    );
    // Exact: the parent's own transactions (a split counts through its parts).
    assert_eq!(
        w.filtered_ids(&format!(r#"categorySlugsExact: ["{parent_slug}"]"#)).await,
        sorted_ids(&[own, own_big, split_into_parent]),
        "no child-only transactions"
    );
    assert_eq!(
        w.filtered_ids(&format!(r#"categorySlugsExact: ["{child_slug}"]"#)).await,
        sorted_ids(&[in_child, split_into_parent, split_only_child])
    );
    // OR-ed among themselves.
    assert_eq!(
        w.filtered_ids(&format!(r#"categorySlugsExact: ["{parent_slug}", "{child_slug}"]"#)).await,
        sorted_ids(&[own, own_big, in_child, split_into_parent, split_only_child])
    );
    // AND-ed with the rest.
    assert_eq!(
        w.filtered_ids(&format!(r#"categorySlugsExact: ["{parent_slug}"] amountMin: "100""#)).await,
        sorted_ids(&[own_big])
    );
    assert_eq!(
        w.filtered_ids(&format!(r#"categorySlugsExact: ["{parent_slug}"] labelSources: [USER]"#)).await,
        sorted_ids(&[own])
    );
    assert_eq!(
        w.filtered_ids(&format!(r#"categorySlugsExact: ["{parent_slug}"] categorySlugs: ["{child_slug}"]"#)).await,
        sorted_ids(&[split_into_parent]),
        "both category filters must hold"
    );
    // Unknown slug matches nothing; empty list is unconstrained.
    assert!(w.filtered_ids(r#"categorySlugsExact: ["no.such.slug"]"#).await.is_empty());
    assert!(w.filtered_ids(&format!(r#"categorySlugsExact: ["{other_slug}"]"#)).await.is_empty());
    let all = w.filtered_ids("").await;
    assert_eq!(w.filtered_ids("categorySlugsExact: []").await, all);

    // The aggregations honour it too (hand-rolled SQL twins).
    let summary = w
        .run_ok(format!(
            r#"{{ cashflowSummary(filter: {{ categorySlugsExact: ["{parent_slug}"] startDate: "2024-07-01" endDate: "2024-07-31" }}, granularity: MONTH) {{
                buckets {{ spending transactionCount }} }} }}"#
        ))
        .await;
    let bucket = &summary["cashflowSummary"]["buckets"][0];
    assert_eq!(bucket["transactionCount"], 3);
    assert_eq!(Decimal::from_str(bucket["spending"].as_str().unwrap()).unwrap(), Decimal::from(350));
    let graph = w
        .run_ok(format!(
            r#"{{ cashflowGraph(filter: {{ categorySlugsExact: ["{parent_slug}"] startDate: "2024-07-01" endDate: "2024-07-31" }}) {{ links {{ value }} }} }}"#
        ))
        .await;
    let total: Decimal = graph["cashflowGraph"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| Decimal::from_str(l["value"].as_str().unwrap()).unwrap())
        .sum();
    assert!(total > Decimal::ZERO);

    // Recurring overview and the breakdown take the filter without error.
    w.run_ok(format!(
        r#"{{ recurringSeries(filter: {{ categorySlugsExact: ["{parent_slug}"] }}) {{ series {{ id }} }} }}"#
    ))
    .await;
    let breakdown = w
        .run_ok(format!(
            r#"{{ categoryBreakdown(filter: {{ categorySlugsExact: ["{parent_slug}"] }}, level: 2) {{ rows {{ category {{ slug }} amount }} }} }}"#
        ))
        .await;
    let rows = breakdown["categoryBreakdown"]["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "{breakdown}");

    // A bulk edit driven by the exact filter touches only the parent's own.
    let data = w
        .run_ok(format!(
            r#"mutation {{ setTransactionsTags(filter: {{ categorySlugsExact: ["{parent_slug}"] }}, tags: ["exact"]) {{ matched applied }} }}"#
        ))
        .await;
    assert_eq!(data["setTransactionsTags"]["matched"], 3);
    assert_eq!(w.tags(own).await, vec!["exact"]);
    assert!(w.tags(in_child).await.is_empty());
}

// ---------------------------------------------------------------------------
// Transaction links (reimbursements)
// ---------------------------------------------------------------------------

impl World {
    async fn tx_on(&self, date: &str, amount: &str, name: &str) -> Uuid {
        common::seed_transaction(&self.db, self.account, date, amount, Some(name)).await
    }

    async fn link_ok(&self, expense: &[Uuid], offset: &[Uuid]) -> serde_json::Value {
        let data = self
            .run_ok(format!(
                r#"mutation {{ createTransactionLink(input: {{ expenseIds: [{}], offsetIds: [{}], note: "dentist" }}) {{
                    id status expenseTotal offsetTotal reimbursed net surplus missingMembers
                    members {{ transactionId role allocated remaining }} }} }}"#,
                id_list(expense),
                id_list(offset)
            ))
            .await;
        data["createTransactionLink"].clone()
    }

    async fn link_of(&self, tx: Uuid) -> serde_json::Value {
        let data = self
            .run_ok(format!(
                r#"{{ transactions(filter: {{ transactionIds: ["{tx}"] }}) {{ items {{ id link {{
                    id status reimbursed net surplus
                    members {{ transactionId role allocated remaining counterpartyName }} }} }} }} }}"#
            ))
            .await;
        data["transactions"]["items"][0]["link"].clone()
    }

    async fn run_err_code(&self, query: String) -> String {
        let response = self.run(query).await;
        let error = response.errors.first().expect("expected an error");
        error
            .extensions
            .as_ref()
            .and_then(|e| e.get("code"))
            .map(|c| c.to_string())
            .unwrap_or_else(|| error.message.clone())
    }

    fn amount(value: &serde_json::Value) -> Decimal {
        Decimal::from_str(value.as_str().unwrap()).unwrap()
    }
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[tokio::test]
async fn a_partial_reimbursement_is_visible_from_both_sides_and_not_rounded_to_reimbursed() {
    let w = world().await;
    let bill = w.tx_on("2024-09-10", "-1000.00", "Praxis Dr. Mueller").await;
    let back = w.tx_on("2024-10-15", "600.00", "Krankenkasse").await;

    let created = w.link_ok(&[bill], &[back]).await;
    assert_eq!(created["status"], "PARTIAL");
    assert_eq!(World::amount(&created["reimbursed"]), dec("600"));
    assert_eq!(World::amount(&created["net"]), dec("400"));

    // The expense side shows what came back and what it still costs...
    let on_bill = w.link_of(bill).await;
    assert_eq!(on_bill["status"], "PARTIAL");
    let mine = on_bill["members"].as_array().unwrap().iter().find(|m| m["transactionId"] == bill.to_string()).unwrap();
    assert_eq!((World::amount(&mine["allocated"]), World::amount(&mine["remaining"])), (dec("600"), dec("400")));
    // ...and the reimbursement side names the expense it offsets.
    let on_back = w.link_of(back).await;
    assert_eq!(on_back["id"], on_bill["id"]);
    let other = on_back["members"].as_array().unwrap().iter().find(|m| m["role"] == "EXPENSE").unwrap();
    assert_eq!(other["counterpartyName"], "Praxis Dr. Mueller");
}

#[tokio::test]
async fn one_reimbursement_spread_over_several_expenses_and_the_reverse() {
    let w = world().await;
    // Many expenses, one reimbursement.
    let (a, b) = (w.tx_on("2024-09-01", "-100.00", "A").await, w.tx_on("2024-09-02", "-300.00", "B").await);
    let back = w.tx_on("2024-09-20", "200.00", "Friend").await;
    let created = w.link_ok(&[a, b], &[back]).await;
    assert_eq!(created["status"], "PARTIAL");
    let shares: Vec<(String, Decimal)> = created["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["transactionId"].as_str().unwrap().to_string(), World::amount(&m["allocated"])))
        .collect();
    assert!(shares.contains(&(a.to_string(), dec("50"))));
    assert!(shares.contains(&(b.to_string(), dec("150"))));

    // One expense, several reimbursements.
    let bill = w.tx_on("2024-09-01", "-900.00", "Hotel").await;
    let (r1, r2) = (w.tx_on("2024-09-05", "300.00", "P1").await, w.tx_on("2024-09-06", "300.00", "P2").await);
    let created = w.link_ok(&[bill], &[r1, r2]).await;
    assert_eq!((World::amount(&created["reimbursed"]), World::amount(&created["net"])), (dec("600"), dec("300")));
    assert_eq!(w.link_of(r2).await["id"], created["id"]);
}

#[tokio::test]
async fn over_reimbursement_caps_the_offset_and_reports_the_surplus() {
    let w = world().await;
    let bill = w.tx_on("2024-09-01", "-50.00", "Dinner").await;
    let back = w.tx_on("2024-09-02", "80.00", "Friends").await;
    let created = w.link_ok(&[bill], &[back]).await;
    assert_eq!(created["status"], "OVER");
    assert_eq!(World::amount(&created["reimbursed"]), dec("50"));
    assert_eq!(World::amount(&created["net"]), dec("0"));
    assert_eq!(World::amount(&created["surplus"]), dec("30"));
}

#[tokio::test]
async fn linking_never_rewrites_a_period_and_unlinking_restores_the_view_exactly() {
    let w = world().await;
    let bill = w.tx_on("2024-09-10", "-1000.00", "Praxis").await;
    let back = w.tx_on("2024-10-15", "600.00", "Krankenkasse").await;

    let summary = |from: &str, to: &str| {
        format!(
            r#"{{ cashflowSummary(filter: {{ startDate: "{from}", endDate: "{to}" }}, granularity: MONTH) {{
                total {{ income spending net transactionCount }} }} }}"#
        )
    };
    let september_before = w.run_ok(summary("2024-09-01", "2024-09-30")).await;
    let october_before = w.run_ok(summary("2024-10-01", "2024-10-31")).await;
    let list_before = w.run_ok(format!(r#"{{ transactions(filter: {{ transactionIds: ["{bill}", "{back}"] }}, sort: {{ field: AMOUNT }}) {{ items {{ id amount link {{ id }} }} }} }}"#)).await;

    let created = w.link_ok(&[bill], &[back]).await;
    // September still shows the money that left; October the money that came.
    assert_eq!(w.run_ok(summary("2024-09-01", "2024-09-30")).await, september_before);
    assert_eq!(w.run_ok(summary("2024-10-01", "2024-10-31")).await, october_before);

    // The netted figure is a separate number, on request.
    let data = w
        .run_ok(r#"{ reimbursementSummary(filter: { startDate: "2024-09-01", endDate: "2024-09-30" }) {
            expenseTotal reimbursed net linkedCount partiallyReimbursedCount } }"#.to_string())
        .await;
    let net = &data["reimbursementSummary"];
    assert_eq!(World::amount(&net["expenseTotal"]), dec("1000"));
    assert_eq!(World::amount(&net["reimbursed"]), dec("600"));
    assert_eq!(World::amount(&net["net"]), dec("400"));
    assert_eq!((net["linkedCount"].as_i64(), net["partiallyReimbursedCount"].as_i64()), (Some(1), Some(1)));
    // October holds only the reimbursement, which is not an expense there.
    let october = w
        .run_ok(r#"{ reimbursementSummary(filter: { startDate: "2024-10-01", endDate: "2024-10-31" }) { linkedCount } }"#.to_string())
        .await;
    assert_eq!(october["reimbursementSummary"]["linkedCount"], 0);

    // Removing the link puts everything back, byte for byte.
    let removed = w
        .run_ok(format!(r#"mutation {{ removeTransactionLink(id: "{}") }}"#, created["id"].as_str().unwrap()))
        .await;
    assert_eq!(removed["removeTransactionLink"], true);
    assert!(w.link_of(bill).await.is_null() && w.link_of(back).await.is_null());
    assert_eq!(w.run_ok(summary("2024-09-01", "2024-09-30")).await, september_before);
    let list_after = w.run_ok(format!(r#"{{ transactions(filter: {{ transactionIds: ["{bill}", "{back}"] }}, sort: {{ field: AMOUNT }}) {{ items {{ id amount link {{ id }} }} }} }}"#)).await;
    assert_eq!(list_after, list_before);
    let again = w.run_ok(format!(r#"mutation {{ removeTransactionLink(id: "{}") }}"#, created["id"].as_str().unwrap())).await;
    assert_eq!(again["removeTransactionLink"], false, "removing twice reports that nothing existed");

    // The tombstone reached the topic and the row is gone.
    use entity::entities::transaction_link;
    let id: Uuid = created["id"].as_str().unwrap().parse().unwrap();
    assert!(transaction_link::Entity::find_by_id(id).one(w.db.as_ref()).await.unwrap().is_none());
}

#[tokio::test]
async fn the_reimbursement_filter_leaves_out_reimbursements_but_never_genuine_income() {
    let w = world().await;
    let salary = w.tx_on("2024-09-25", "2000.00", "Employer").await;
    let bill = w.tx_on("2024-09-10", "-1000.00", "Praxis").await;
    let back = w.tx_on("2024-09-28", "600.00", "Krankenkasse").await;
    w.link_ok(&[bill], &[back]).await;
    let (cat, _slug) = common::seed_category(&w.db, "linkinc", "Link income", "income").await;
    for tx in [salary, back] {
        common::seed_transaction_label(&w.db, tx, Some(cat), "user", "resolved").await;
    }

    let range = r#"startDate: "2024-09-01", endDate: "2024-09-30""#;
    let income = |extra: &str| format!(r#"{{ cashflowSummary(filter: {{ {range}, direction: INCOME {extra} }}, granularity: MONTH) {{ total {{ income transactionCount }} }} }}"#);
    let total = |v: &serde_json::Value| (World::amount(&v["cashflowSummary"]["total"]["income"]), v["cashflowSummary"]["total"]["transactionCount"].as_i64().unwrap());

    // Default: included. The view a user already reviewed does not change.
    assert_eq!(total(&w.run_ok(income("")).await), (dec("2600"), 2));
    assert_eq!(total(&w.run_ok(income(", reimbursements: INCLUDE")).await), (dec("2600"), 2));
    // Excluded: the salary stays.
    assert_eq!(total(&w.run_ok(income(", reimbursements: EXCLUDE")).await), (dec("2000"), 1));
    assert_eq!(total(&w.run_ok(income(", reimbursements: ONLY")).await), (dec("600"), 1));

    // The list.
    let ids = |v: &serde_json::Value| -> Vec<String> {
        v["transactions"]["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect()
    };
    let list = |extra: &str| format!(r#"{{ transactions(filter: {{ {range}, direction: INCOME {extra} }}) {{ items {{ id }} totalCount }} }}"#);
    assert_eq!(ids(&w.run_ok(list(", reimbursements: EXCLUDE")).await), vec![salary.to_string()]);
    assert_eq!(ids(&w.run_ok(list(", reimbursements: ONLY")).await), vec![back.to_string()]);
    assert_eq!(w.run_ok(list("")).await["transactions"]["totalCount"], 2);

    // The cashflow graph (a second hand-rolled SQL twin).
    let graph = |extra: &str| format!(r#"{{ cashflowGraph(filter: {{ {range} {extra} }}) {{ links {{ value }} }} }}"#);
    let graph_total = |v: &serde_json::Value| -> Decimal {
        v["cashflowGraph"]["links"].as_array().unwrap().iter().map(|l| World::amount(&l["value"])).sum()
    };
    assert!(graph_total(&w.run_ok(graph(", reimbursements: EXCLUDE")).await) < graph_total(&w.run_ok(graph("")).await));

    // The category breakdown.
    let breakdown = |extra: &str| format!(r#"{{ categoryBreakdown(filter: {{ {range} {extra} }}, kind: INCOME) {{ rows {{ category {{ slug }} amount transactionCount }} }} }}"#);
    let row = |v: &serde_json::Value| -> (Decimal, i64) {
        let r = v["categoryBreakdown"]["rows"].as_array().unwrap().iter().map(|r| r.clone()).next().expect("one income row");
        (World::amount(&r["amount"]), r["transactionCount"].as_i64().unwrap())
    };
    assert_eq!(row(&w.run_ok(breakdown("")).await), (dec("2600"), 2));
    assert_eq!(row(&w.run_ok(breakdown(", reimbursements: EXCLUDE")).await), (dec("2000"), 1));

    // Recurring accepts the filter (a filter that matches nothing yields no series).
    let recurring = w.run_ok(format!(r#"{{ recurringSeries(filter: {{ {range}, reimbursements: ONLY }}) {{ series {{ id }} }} }}"#)).await;
    assert!(recurring["recurringSeries"]["series"].as_array().unwrap().is_empty());

    // The bulk mutations honour it too: tagging "only reimbursements" touches just that one.
    let tagged = w
        .run_ok(format!(r#"mutation {{ setTransactionsTags(filter: {{ {range}, direction: INCOME, reimbursements: ONLY }}, tags: ["back"]) {{ matched applied failed splitsCleared }} }}"#))
        .await;
    assert_eq!(tagged["setTransactionsTags"]["matched"], 1);
    assert_eq!(w.tags(back).await, vec!["back"]);
    assert!(w.tags(salary).await.is_empty());
}

#[tokio::test]
async fn a_link_cannot_reach_another_users_transactions_nor_be_managed_by_them() {
    let w = world().await;
    let mine = w.tx_on("2024-09-10", "-100.00", "Shop").await;
    let back = w.tx_on("2024-09-20", "100.00", "Friend").await;

    let (other_user, other_name) = common::seed_user(&w.db, "link-other", "pw").await;
    let other_account = common::seed_account(&w.db, "EUR", "Other").await;
    common::link(&w.db, other_user, other_account).await;
    let theirs = common::seed_transaction(&w.db, other_account, "2024-09-20", "100.00", Some("Theirs")).await;

    // A link to a transaction in an account the caller does not own is refused, as if it did not exist.
    let q = format!(
        r#"mutation {{ createTransactionLink(input: {{ expenseIds: ["{mine}"], offsetIds: ["{theirs}"] }}) {{ id }} }}"#
    );
    let response = w.run(q).await;
    assert!(!response.errors.is_empty());
    use entity::entities::transaction_link_member;
    let leaked = transaction_link_member::Entity::find()
        .filter(transaction_link_member::Column::TransactionId.eq(theirs))
        .all(w.db.as_ref())
        .await
        .unwrap();
    assert!(leaked.is_empty());

    // A link of mine is invisible to, and unmanageable by, the other user.
    let created = w.link_ok(&[mine], &[back]).await;
    let link_id = created["id"].as_str().unwrap().to_string();
    let intruder = AuthenticatedUser { user_id: other_user, username: other_name, account_ids: vec![other_account] };
    let run_as_intruder = |q: String| {
        let schema = w.schema.clone();
        let intruder = intruder.clone();
        async move { schema.execute(authed(Request::new(q), &intruder)).await }
    };
    let seen = run_as_intruder(format!(r#"{{ transactionLink(id: "{link_id}") {{ id }} }}"#)).await;
    assert!(seen.errors.is_empty());
    assert_eq!(seen.data.into_json().unwrap()["transactionLink"], serde_json::Value::Null);
    let removed = run_as_intruder(format!(r#"mutation {{ removeTransactionLink(id: "{link_id}") }}"#)).await;
    assert_eq!(removed.data.into_json().unwrap()["removeTransactionLink"], false);
    let hijack = run_as_intruder(format!(
        r#"mutation {{ updateTransactionLink(id: "{link_id}", input: {{ expenseIds: ["{theirs}"], offsetIds: ["{theirs}"] }}) {{ id }} }}"#
    ))
    .await;
    assert!(!hijack.errors.is_empty());
    let list = run_as_intruder(r#"{ transactionLinks { id } }"#.to_string()).await;
    assert!(list.data.into_json().unwrap()["transactionLinks"].as_array().unwrap().is_empty());
    assert!(!w.link_of(mine).await.is_null(), "my link is untouched");
}

#[tokio::test]
async fn link_rules_are_enforced_at_write_time() {
    let w = world().await;
    let bill = w.tx_on("2024-09-10", "-100.00", "Shop").await;
    let other_bill = w.tx_on("2024-09-11", "-50.00", "Shop 2").await;
    let back = w.tx_on("2024-09-20", "100.00", "Friend").await;
    let back2 = w.tx_on("2024-09-21", "20.00", "Friend 2").await;

    let create = |e: &[Uuid], o: &[Uuid]| {
        format!(r#"mutation {{ createTransactionLink(input: {{ expenseIds: [{}], offsetIds: [{}] }}) {{ id }} }}"#, id_list(e), id_list(o))
    };
    // Wrong sign on a side.
    assert_eq!(w.run_err_code(create(&[back], &[bill])).await, "\"VALIDATION\"");
    // Nothing on one side.
    assert_eq!(w.run_err_code(create(&[bill], &[])).await, "\"VALIDATION\"");

    // One transaction, one link.
    let created = w.link_ok(&[bill], &[back]).await;
    assert_eq!(w.run_err_code(create(&[other_bill], &[back])).await, "\"ALREADY_LINKED\"");

    // Updating replaces the members: a second reimbursement joins the same link.
    let id = created["id"].as_str().unwrap();
    let updated = w
        .run_ok(format!(
            r#"mutation {{ updateTransactionLink(id: "{id}", input: {{ expenseIds: ["{bill}"], offsetIds: ["{back}", "{back2}"] }}) {{ id status offsetTotal }} }}"#
        ))
        .await;
    assert_eq!(updated["updateTransactionLink"]["id"], id);
    assert_eq!(World::amount(&updated["updateTransactionLink"]["offsetTotal"]), dec("120"));
    assert_eq!(updated["updateTransactionLink"]["status"], "OVER");
}

#[tokio::test]
async fn candidates_rank_the_likely_counterpart_first_and_skip_linked_ones() {
    let w = world().await;
    let bill = w.tx_on("2024-09-10", "-1000.00", "Praxis").await;
    let likely = w.tx_on("2024-09-25", "980.00", "Krankenkasse").await;
    let unrelated = w.tx_on("2024-09-26", "12.00", "Refund shop").await;
    let wrong_sign = w.tx_on("2024-09-12", "-1000.00", "Another bill").await;
    let linked = w.tx_on("2024-09-11", "1000.00", "Already used").await;
    let linked_bill = w.tx_on("2024-09-01", "-1000.00", "Other bill").await;
    w.link_ok(&[linked_bill], &[linked]).await;

    let data = w
        .run_ok(format!(r#"{{ linkCandidates(transactionId: "{bill}") {{ score transaction {{ id }} }} }}"#))
        .await;
    let ids: Vec<String> = data["linkCandidates"].as_array().unwrap().iter().map(|c| c["transaction"]["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids.first().map(String::as_str), Some(likely.to_string().as_str()));
    assert!(ids.contains(&unrelated.to_string()));
    assert!(!ids.contains(&wrong_sign.to_string()), "same sign cannot offset");
    assert!(!ids.contains(&linked.to_string()), "already linked");
    assert!(!ids.contains(&bill.to_string()));

    // Searching widens to the whole history by name.
    let found = w
        .run_ok(format!(r#"{{ linkCandidates(transactionId: "{bill}", search: "krankenkasse") {{ transaction {{ id }} }} }}"#))
        .await;
    assert_eq!(found["linkCandidates"].as_array().unwrap().len(), 1);
}
