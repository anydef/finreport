//! `category-reslug` end to end: a broken category with a label, an override
//! (tags + recurring), a split part and a rule, repaired against a throwaway
//! Kafka and the shared `finreport-wp4-pg` Postgres (`common::db`). Every slug
//! carries a random suffix and each run is scoped with `only`, so the shared
//! database's other tests are neither affected nor asserted on.
#![cfg(feature = "integration")]

mod common;
#[path = "support/mod.rs"]
mod support;

use chrono::{Duration, Utc};
use entity::entities::{
    category, rule, transaction_label, transaction_split, transaction_user_label,
};
use rust_decimal::Decimal;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use support::TestKafka;
use uuid::Uuid;
use webapp::category_reslug::{Action, Options, run};
use webapp::kafka::envelope::transaction_uuid;
use webapp::kafka::labeling::{
    CURRENT_SCHEMA_VERSION, RuleConditions, RuleOrigin, RuleRecord, RuleState, SplitPart,
    TOPIC_CATEGORY, TOPIC_RULE, TOPIC_USER_LABEL, UserLabelRecord, category_uuid,
};
use webapp::kafka::producer::EventPublisher;
use webapp::kafka::scan::scan_topic;

async fn insert_category(db: &DatabaseConnection, slug: &str, parent: Option<&str>) {
    category::ActiveModel {
        id: Set(category_uuid(slug)),
        slug: Set(slug.to_string()),
        parent_id: Set(parent.map(category_uuid)),
        name: Set(format!("name of {slug}")),
        kind: Set("expense".to_string()),
        depth: Set(slug.matches('.').count() as i16 + 1),
        sort_order: Set(0),
        archived: Set(false),
        origin: Set("user".to_string()),
        owner_user_id: Set(None),
        revision: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert category");
}

fn headers() -> rdkafka::message::OwnedHeaders {
    use rdkafka::message::{Header, OwnedHeaders};
    OwnedHeaders::new().insert(Header {
        key: "origin",
        value: Some("user"),
    })
}

struct Seeded {
    suffix: String,
    parent: String,
    broken: String,
    fixed: String,
    override_tx: Uuid,
    split_tx: Uuid,
    rule_id: Uuid,
}

fn user_label(ext: &str, category: Option<&str>, parts: &[(&str, &str)]) -> UserLabelRecord {
    UserLabelRecord {
        schema_version: 2,
        source: "test".into(),
        external_id: ext.into(),
        category_slug: category.map(String::from),
        parts: parts
            .iter()
            .enumerate()
            .map(|(i, (amount, slug))| SplitPart {
                index: i as i32,
                amount: amount.parse::<Decimal>().unwrap(),
                category_slug: slug.to_string(),
            })
            .collect(),
        tags: vec!["holiday".into(), "kids".into()],
        recurring: Some(true),
        revision: Utc::now() - Duration::hours(1),
        note: Some("my note".into()),
    }
}

/// A broken category plus the four things that reference it, on both sides
/// (Kafka for the source of truth, Postgres for the projection).
async fn seed(db: &DatabaseConnection, publisher: &EventPublisher, correct_exists: bool) -> Seeded {
    let suffix = Uuid::new_v4().simple().to_string()[..8].to_string();
    let parent = format!("par_{suffix}");
    let broken = format!("leaf_{suffix}");
    let fixed = format!("{parent}.leaf_{suffix}");
    insert_category(db, &parent, None).await;
    insert_category(db, &broken, Some(&parent)).await;
    if correct_exists {
        insert_category(db, &fixed, Some(&parent)).await;
    }

    let account = common::seed_account(db, "EUR", "reslug").await;
    let label_tx = common::seed_transaction(db, account, "2024-07-01", "-10.00", Some("Shop")).await;
    common::seed_transaction_label(db, label_tx, Some(category_uuid(&broken)), "llm", "resolved").await;

    let override_ext = format!("override-{suffix}");
    let split_ext = format!("split-{suffix}");
    let override_tx = transaction_uuid("test", &override_ext);
    let split_tx = transaction_uuid("test", &split_ext);
    for (ext, tx, rec) in [
        (&override_ext, override_tx, user_label(&override_ext, Some(&broken), &[])),
        (
            &split_ext,
            split_tx,
            user_label(&split_ext, None, &[("-6.00", &broken), ("-4.00", "food")]),
        ),
    ] {
        publisher
            .publish_with_headers(
                TOPIC_USER_LABEL,
                &format!("test:{ext}"),
                &serde_json::to_vec(&rec).unwrap(),
                headers(),
            )
            .await
            .unwrap();
        webapp::projection::labeling::project_user_label(db, tx, Some(rec))
            .await
            .unwrap();
    }

    let rule_id = Uuid::new_v4();
    let rule_record = RuleRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        id: rule_id,
        name: "shop rule".into(),
        category_slug: broken.clone(),
        conditions: RuleConditions {
            counterparty_key: Some(format!("shop{suffix}")),
            description_contains: Some("abo".into()),
            ..Default::default()
        },
        priority: 7,
        state: RuleState::Active,
        origin: RuleOrigin::Learned,
        auto_approved: true,
        user_touched: true,
        confidence: Some(0.9),
        evidence: None,
        created_at: Utc::now() - Duration::days(1),
        revision: Utc::now() - Duration::hours(1),
    };
    publisher
        .publish_with_headers(
            TOPIC_RULE,
            &rule_id.to_string(),
            &serde_json::to_vec(&rule_record).unwrap(),
            headers(),
        )
        .await
        .unwrap();
    webapp::projection::labeling::project_rule(db, rule_id, Some(rule_record))
        .await
        .unwrap();

    Seeded {
        suffix,
        parent,
        broken,
        fixed,
        override_tx,
        split_tx,
        rule_id,
    }
}

fn opts(s: &Seeded, dry_run: bool) -> Options {
    Options {
        dry_run,
        only: vec![s.broken.clone()],
    }
}

async fn category_exists(db: &DatabaseConnection, slug: &str) -> bool {
    category::Entity::find_by_id(category_uuid(slug))
        .one(db)
        .await
        .unwrap()
        .is_some()
}

fn latest_json(brokers: &str, topic: &str, key: &str) -> Option<serde_json::Value> {
    scan_topic(brokers, topic)
        .unwrap()
        .into_iter()
        .filter(|r| r.key.as_deref() == Some(key))
        .next_back()
        .and_then(|r| r.payload)
        .map(|p| serde_json::from_slice(&p).unwrap())
}

#[tokio::test]
async fn repairs_a_broken_category_end_to_end_and_a_rerun_is_a_noop() {
    let kafka = TestKafka::start().await;
    let brokers = kafka.bootstrap_servers().to_string();
    let db = common::db().await;
    let publisher = EventPublisher::connect(&brokers).unwrap();
    let s = seed(&db, &publisher, false).await;

    // Dry run: reports, publishes and changes nothing.
    let before = scan_topic(&brokers, TOPIC_USER_LABEL).unwrap().len();
    let dry = run(&db, &brokers, None, &opts(&s, true)).await.unwrap();
    assert_eq!(dry.len(), 1);
    assert_eq!(dry[0].action, Action::Create);
    assert_eq!(dry[0].new_slug, s.fixed);
    assert_eq!(
        (dry[0].labels, dry[0].overrides, dry[0].splits, dry[0].rules),
        (1, 1, 1, 1)
    );
    assert_eq!(scan_topic(&brokers, TOPIC_USER_LABEL).unwrap().len(), before);
    assert!(scan_topic(&brokers, TOPIC_CATEGORY).unwrap().is_empty());
    assert!(category_exists(&db, &s.broken).await);
    assert!(!category_exists(&db, &s.fixed).await);

    // Real run.
    let first = run(&db, &brokers, Some(&publisher), &opts(&s, false))
        .await
        .unwrap();
    assert_eq!(first, dry);

    // Postgres: new category in the tree, old one gone.
    let new_cat = category::Entity::find_by_id(category_uuid(&s.fixed))
        .one(&*db)
        .await
        .unwrap()
        .expect("corrected category exists");
    assert_eq!(new_cat.parent_id, Some(category_uuid(&s.parent)));
    assert_eq!(new_cat.name, format!("name of {}", s.broken));
    assert_eq!((new_cat.kind.as_str(), new_cat.origin.as_str(), new_cat.depth), ("expense", "user", 2));
    assert!(!category_exists(&db, &s.broken).await);

    // Override: category moved, tags/recurring/note intact.
    let ul = transaction_user_label::Entity::find_by_id(s.override_tx)
        .one(&*db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ul.category_id, Some(category_uuid(&s.fixed)));
    assert_eq!(ul.recurring, Some(true));
    assert_eq!(ul.note.as_deref(), Some("my note"));
    let key = format!("test:override-{}", s.suffix);
    let rec = latest_json(&brokers, TOPIC_USER_LABEL, &key).unwrap();
    assert_eq!(rec["category_slug"], s.fixed.as_str());
    assert_eq!(rec["tags"], serde_json::json!(["holiday", "kids"]));
    assert_eq!(rec["recurring"], true);
    assert_eq!(rec["note"], "my note");

    // Split: the part on the broken slug moved, the other untouched.
    let splits = transaction_split::Entity::find()
        .filter(transaction_split::Column::TransactionId.eq(s.split_tx))
        .all(&*db)
        .await
        .unwrap();
    assert_eq!(splits.len(), 2);
    assert!(splits.iter().any(|p| p.category_id == category_uuid(&s.fixed)));
    assert!(splits.iter().any(|p| p.category_id == category_uuid("food")));
    let rec = latest_json(&brokers, TOPIC_USER_LABEL, &format!("test:split-{}", s.suffix)).unwrap();
    assert_eq!(rec["parts"][0]["category_slug"], s.fixed.as_str());
    assert_eq!(rec["parts"][1]["category_slug"], "food");
    assert_eq!(rec["tags"], serde_json::json!(["holiday", "kids"]));

    // Rule: category moved, conditions and the rest preserved.
    let r = rule::Entity::find_by_id(s.rule_id).one(&*db).await.unwrap().unwrap();
    assert_eq!(r.category_id, category_uuid(&s.fixed));
    assert_eq!(r.conditions["counterparty_key"], format!("shop{}", s.suffix));
    assert_eq!(r.conditions["description_contains"], "abo");
    assert_eq!((r.priority, r.user_touched, r.origin.as_str()), (7, true, "learned"));
    let rec = latest_json(&brokers, TOPIC_RULE, &s.rule_id.to_string()).unwrap();
    assert_eq!(rec["category_slug"], s.fixed.as_str());
    assert_eq!(rec["conditions"]["description_contains"], "abo");

    // Stale labels are gone so the sweep re-resolves them.
    let stale = transaction_label::Entity::find()
        .filter(transaction_label::Column::CategoryId.eq(category_uuid(&s.broken)))
        .all(&*db)
        .await
        .unwrap();
    assert!(stale.is_empty());

    // Category topic: the corrected record, then a tombstone for the old id.
    let cat_records = scan_topic(&brokers, TOPIC_CATEGORY).unwrap();
    let new_key = category_uuid(&s.fixed).to_string();
    let old_key = category_uuid(&s.broken).to_string();
    let new_pos = cat_records.iter().position(|r| r.key.as_deref() == Some(&new_key) && r.payload.is_some()).unwrap();
    let tomb_pos = cat_records.iter().position(|r| r.key.as_deref() == Some(&old_key) && r.payload.is_none()).unwrap();
    assert!(new_pos < tomb_pos, "new category must be published before the old is tombstoned");
    let new_json: serde_json::Value = serde_json::from_slice(cat_records[new_pos].payload.as_ref().unwrap()).unwrap();
    assert_eq!(new_json["slug"], s.fixed.as_str());
    assert_eq!(new_json["parent_slug"], s.parent.as_str());

    // Idempotent: nothing left to do, nothing published.
    let published = scan_topic(&brokers, TOPIC_USER_LABEL).unwrap().len();
    let again = run(&db, &brokers, Some(&publisher), &opts(&s, false)).await.unwrap();
    assert!(again.is_empty());
    assert_eq!(scan_topic(&brokers, TOPIC_USER_LABEL).unwrap().len(), published);
}

#[tokio::test]
async fn merges_onto_an_existing_correct_category() {
    let kafka = TestKafka::start().await;
    let brokers = kafka.bootstrap_servers().to_string();
    let db = common::db().await;
    let publisher = EventPublisher::connect(&brokers).unwrap();
    let s = seed(&db, &publisher, true).await;

    let reports = run(&db, &brokers, Some(&publisher), &opts(&s, false)).await.unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(
        reports[0].action,
        Action::Merge { existing_id: category_uuid(&s.fixed) }
    );

    // No new category record was published for the target; the old one is tombstoned.
    let cat_records = scan_topic(&brokers, TOPIC_CATEGORY).unwrap();
    let new_key = category_uuid(&s.fixed).to_string();
    assert!(cat_records.iter().all(|r| r.key.as_deref() != Some(&new_key)));
    assert!(!category_exists(&db, &s.broken).await);
    assert!(category_exists(&db, &s.fixed).await);

    let ul = transaction_user_label::Entity::find_by_id(s.override_tx).one(&*db).await.unwrap().unwrap();
    assert_eq!(ul.category_id, Some(category_uuid(&s.fixed)));
}
