//! WP3 labeler integration tests (§10 "done when", the WP3-owned slice:
//! fixture replay → resolution → dual-write → compare-before-publish →
//! idempotent replay, cost guard, category seeding). Drives
//! `labeling::processor` directly against a real Postgres + Kafka pair
//! (WP6's shared `support::TestPostgres`/`TestKafka` harness), the same way
//! `projector_kafka.rs`/`projector_postgres.rs` do for the original
//! projector.
//!
//! WP2's `normalize`/`fingerprint`/`rules::most_specific_match`/`learn::consider`
//! remain `todo!()` stubs while WP2 is implemented in parallel, so every test
//! here injects [`LabelingOps::fake`] instead of [`LabelingOps::real`] — this
//! exercises WP3's own resolve/compare/publish/project machinery, cost guard
//! and category seeding, never WP2's actual matching/normalization logic.
//! Tests that would need the real WP2 behaviour to assert anything
//! meaningful are `#[ignore = "needs WP2"]`, listed in the task reply.
#![cfg(feature = "integration")]

#[path = "support/mod.rs"]
mod support;

use chrono::Utc;
use entity::entities::{account, transaction, transaction_label};
use rust_decimal::Decimal;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};
use std::str::FromStr;
use support::{TestKafka, TestPostgres};
use uuid::Uuid;
use webapp::kafka::labeling::category_uuid;
use webapp::kafka::producer::EventPublisher;
use webapp::labeling::processor::{label_one_transaction, run_sweep, CostGuard, LabelingOps};
use webapp::projection::labeling::{self as proj, TransactionForLabeling};

/// Minimal account + transaction row, inserted directly (no FK on the five
/// new labeling tables, but `transaction` itself still needs a real
/// `account_id` and the row the labeler's `set_counterparty_key` update
/// targets).
async fn seed_transaction(
    db: &sea_orm::DatabaseConnection,
    external_id: &str,
    counterparty_name: &str,
) -> TransactionForLabeling {
    let account_id = Uuid::new_v4();
    account::ActiveModel {
        id: Set(account_id),
        source: Set("comdirect".to_string()),
        external_id: Set("ACC1".to_string()),
        display_id: Set(Some("1053820100".to_string())),
        account_type: Set(Some("Girokonto".to_string())),
        iban: Set(Some(format!("DE{external_id}"))),
        bic: Set(Some("COBADEFFXXX".to_string())),
        institute: Set(None),
        label: Set(None),
        currency: Set("EUR".to_string()),
        raw_payload: Set(None),
        origin: Set("comdirect".to_string()),
        first_seen_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert test account");

    let txn_id = Uuid::new_v4();
    transaction::ActiveModel {
        id: Set(txn_id),
        account_id: Set(account_id),
        source: Set("comdirect".to_string()),
        external_id: Set(external_id.to_string()),
        booking_date: Set(Utc::now().date_naive()),
        valuta_date: Set(Some(Utc::now().date_naive())),
        booking_status: Set("BOOKED".to_string()),
        amount: Set(Decimal::from_str("-42.00").unwrap()),
        currency: Set("EUR".to_string()),
        counterparty_name: Set(Some(counterparty_name.to_string())),
        counterparty_iban: Set(None),
        description: Set(None),
        transaction_type: Set(None),
        raw_payload: Set(serde_json::json!({})),
        origin: Set("comdirect".to_string()),
        imported_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
        counterparty_key: Set(None),
    }
    .insert(db)
    .await
    .expect("insert test transaction");

    TransactionForLabeling {
        id: txn_id,
        source: "comdirect".to_string(),
        external_id: external_id.to_string(),
        account_id,
        amount: Decimal::from_str("-42.00").unwrap(),
        currency: "EUR".to_string(),
        counterparty_name: Some(counterparty_name.to_string()),
        counterparty_iban: None,
        description: None,
        transaction_type: None,
        counterparty_key: None,
        booking_date: Utc::now().date_naive(),
    }
}

/// A `LabelingOps` whose rule-matcher always comes up empty (so resolution
/// falls through to the LLM/`FakeProvider` tier) and whose learner never
/// fires — enough to exercise WP3's own chain without depending on WP2's
/// real logic for anything's *content*.
fn fakes_with_no_rule_match() -> LabelingOps {
    LabelingOps::fake(
        |counterparty, description| {
            counterparty
                .or(description)
                .unwrap_or("")
                .to_lowercase()
        },
        |_, _, _, counterparty, _, _| format!("fp-{counterparty}"),
        |_rules, _input| None,
        |_observations| None,
    )
}

#[tokio::test]
async fn resolving_a_transaction_with_no_rule_match_publishes_an_llm_label_and_is_idempotent() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url())
        .await
        .expect("connect to test Postgres");
    let publisher =
        EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    // "Fitness First" matches the FakeProvider's deterministic keyword table
    // (categorizer::provider::fake, §2.9) → `food.restaurants` is *not* it;
    // the fake maps a "fitness"/"gym" keyword to a sport/fitness category,
    // whichever slug the catalog below seeds under that keyword table.
    let txn = seed_transaction(&db, "FITNESS-TEST-01", "Fitness First").await;

    // Seed just the categories the FakeProvider's catalog needs to resolve
    // against, instead of running the full category-seed binary.
    // "personal.gym" is the slug the FakeProvider's keyword table maps
    // "fitness first" to (categorizer::provider::fake::KEYWORD_TABLE) — it
    // must exist in the projection for build_label_record's slug lookup to
    // round-trip the deterministic category_id back to a name.
    for slug in ["uncategorized", "personal.gym"] {
        proj::project_category(
            &db,
            category_uuid(slug),
            Some(webapp::kafka::labeling::CategoryRecord {
                schema_version: webapp::kafka::labeling::CURRENT_SCHEMA_VERSION,
                id: category_uuid(slug),
                slug: slug.to_string(),
                parent_slug: None,
                name: slug.to_string(),
                kind: webapp::kafka::labeling::CategoryKind::Expense,
                depth: 1,
                sort_order: 0,
                archived: false,
                origin: webapp::kafka::labeling::CategoryOrigin::Seed,
                owner_user_id: None,
                revision: Utc::now(),
            }),
        )
        .await
        .expect("seed category");
    }

    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let provider = categorizer::provider::fake::FakeProvider::new();
    let ops = fakes_with_no_rule_match();
    let cost_guard = CostGuard::new(10);

    label_one_transaction(
        &db,
        &publisher,
        &ops,
        &provider,
        &catalog,
        "test-prompt-v1",
        0.0,
        &cost_guard,
        &txn,
    )
    .await
    .expect("label_one_transaction should resolve and publish");

    let stored = transaction_label::Entity::find_by_id(txn.id)
        .one(&db)
        .await
        .expect("query transaction_label")
        .expect("a label must have been published for this transaction");
    assert_eq!(stored.label_source, "llm");
    let first_category_id = stored.category_id;

    // Second run over the exact same (deterministic) inputs must be a
    // no-op at the storage layer: compare-before-publish must not treat an
    // unchanged resolution as a change (§2.5).
    let reloaded = proj::find_transaction(&db, txn.id)
        .await
        .expect("reload transaction")
        .expect("transaction still exists");

    label_one_transaction(
        &db,
        &publisher,
        &ops,
        &provider,
        &catalog,
        "test-prompt-v1",
        0.0,
        &cost_guard,
        &reloaded,
    )
    .await
    .expect("second resolve should also succeed");

    let stored_again = transaction_label::Entity::find_by_id(txn.id)
        .one(&db)
        .await
        .expect("query transaction_label")
        .expect("label still present");
    assert_eq!(stored_again.category_id, first_category_id);
    assert_eq!(stored_again.label_source, "llm");
}

#[tokio::test]
async fn the_sweep_labels_every_unlabelled_transaction_it_finds() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url())
        .await
        .expect("connect to test Postgres");
    let publisher =
        EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    let txn = seed_transaction(&db, "SWEEP-TEST-01", "Some Shop").await;

    for slug in ["uncategorized"] {
        proj::project_category(
            &db,
            category_uuid(slug),
            Some(webapp::kafka::labeling::CategoryRecord {
                schema_version: webapp::kafka::labeling::CURRENT_SCHEMA_VERSION,
                id: category_uuid(slug),
                slug: slug.to_string(),
                parent_slug: None,
                name: slug.to_string(),
                kind: webapp::kafka::labeling::CategoryKind::Expense,
                depth: 1,
                sort_order: 0,
                archived: false,
                origin: webapp::kafka::labeling::CategoryOrigin::Seed,
                owner_user_id: None,
                revision: Utc::now(),
            }),
        )
        .await
        .expect("seed category");
    }

    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let provider = categorizer::provider::fake::FakeProvider::new();
    let ops = fakes_with_no_rule_match();
    let cost_guard = CostGuard::new(10);

    let labeled = run_sweep(
        &db,
        &publisher,
        &ops,
        &provider,
        &catalog,
        "test-prompt-v1",
        0.0,
        &cost_guard,
        100,
    )
    .await
    .expect("sweep should succeed");
    assert_eq!(labeled, 1);

    let stored = transaction_label::Entity::find_by_id(txn.id)
        .one(&db)
        .await
        .expect("query transaction_label")
        .expect("sweep must have published a label");
    assert_eq!(stored.label_source, "llm");
}

/// Seeds a `category_id`/`uncategorized` pair plus whatever extra slugs are
/// given, mirroring the inline seeding every other test in this file does.
async fn seed_categories(db: &sea_orm::DatabaseConnection, slugs: &[&str]) {
    for slug in slugs {
        proj::project_category(
            db,
            category_uuid(slug),
            Some(webapp::kafka::labeling::CategoryRecord {
                schema_version: webapp::kafka::labeling::CURRENT_SCHEMA_VERSION,
                id: category_uuid(slug),
                slug: slug.to_string(),
                parent_slug: None,
                name: slug.to_string(),
                kind: webapp::kafka::labeling::CategoryKind::Expense,
                depth: 1,
                sort_order: 0,
                archived: false,
                origin: webapp::kafka::labeling::CategoryOrigin::Seed,
                owner_user_id: None,
                revision: Utc::now(),
            }),
        )
        .await
        .expect("seed category");
    }
}

/// Uses WP2's real `rules::most_specific_match` (via `LabelingOps::real`) to
/// assert a matching active rule wins over the LLM tier — the provider here
/// panics if ever called, so a `rule`-sourced label is the only way this
/// test passes.
#[tokio::test]
async fn a_matching_rule_wins_over_the_llm_tier() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url())
        .await
        .expect("connect to test Postgres");
    let publisher =
        EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    let mut txn = seed_transaction(&db, "RULE-TEST-01", "Lidl Filiale 123").await;
    let normalized_key = webapp::labeling::normalize::normalize(txn.counterparty_name.as_deref(), None);
    proj::set_counterparty_key(&db, txn.id, &normalized_key)
        .await
        .expect("set counterparty key");
    txn.counterparty_key = Some(normalized_key.clone());

    seed_categories(&db, &["uncategorized", "food.groceries"]).await;

    let rule_id = Uuid::new_v4();
    proj::project_rule(
        &db,
        rule_id,
        Some(webapp::kafka::labeling::RuleRecord {
            schema_version: webapp::kafka::labeling::CURRENT_SCHEMA_VERSION,
            id: rule_id,
            name: "lidl-rule".to_string(),
            category_slug: "food.groceries".to_string(),
            conditions: webapp::kafka::labeling::RuleConditions {
                counterparty_key: Some(normalized_key.clone()),
                ..Default::default()
            },
            priority: 0,
            state: webapp::kafka::labeling::RuleState::Active,
            origin: webapp::kafka::labeling::RuleOrigin::User,
            auto_approved: false,
            user_touched: true,
            confidence: None,
            evidence: None,
            created_at: Utc::now(),
            revision: Utc::now(),
        }),
    )
    .await
    .expect("seed rule");

    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let ops = LabelingOps::real(3, 0.9);
    let cost_guard = CostGuard::new(10);

    struct PanicProvider;
    #[async_trait::async_trait]
    impl categorizer::provider::LabelProvider for PanicProvider {
        fn id(&self) -> &'static str {
            "panic"
        }
        fn model(&self) -> &str {
            "panic"
        }
        async fn suggest(
            &self,
            _req: &categorizer::provider::LabelRequest<'_>,
        ) -> Result<categorizer::provider::LabelSuggestion, categorizer::provider::ProviderError> {
            panic!("a matching rule must short-circuit before ever calling the provider");
        }
    }

    label_one_transaction(
        &db,
        &publisher,
        &ops,
        &PanicProvider,
        &catalog,
        "test-prompt-v1",
        0.0,
        &cost_guard,
        &txn,
    )
    .await
    .expect("label_one_transaction should resolve via the rule and publish");

    let stored = transaction_label::Entity::find_by_id(txn.id)
        .one(&db)
        .await
        .expect("query transaction_label")
        .expect("rule match must have published a label");
    assert_eq!(stored.label_source, "rule");
    assert_eq!(stored.rule_id, Some(rule_id));
}

/// Uses WP2's real `learn::consider` (via `LabelingOps::real`) to assert a
/// rule is actually proposed after enough repeated, agreeing LLM
/// observations for the same `counterparty_key` (§2.8).
#[tokio::test]
async fn repeated_llm_agreement_learns_a_rule() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url())
        .await
        .expect("connect to test Postgres");
    let publisher =
        EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    // "Fitness First" deterministically maps to "personal.gym" under the
    // FakeProvider's keyword table (§2.9), so three separate transactions
    // for the same counterparty all agree on the same category.
    seed_categories(&db, &["uncategorized", "personal.gym"]).await;
    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let provider = categorizer::provider::fake::FakeProvider::new();
    // min_observations=3, auto_approve_threshold=0.9: FakeProvider's
    // confidence for a keyword hit is high enough to clear the bar once
    // three agreeing observations land (§2.8 "confidence = min of the
    // underlying confidences").
    let ops = LabelingOps::real(3, 0.9);
    let cost_guard = CostGuard::new(10);

    for i in 0..3 {
        let txn = seed_transaction(&db, &format!("LEARN-TEST-{i:02}"), "Fitness First").await;
        label_one_transaction(
            &db,
            &publisher,
            &ops,
            &provider,
            &catalog,
            "test-prompt-v1",
            0.0,
            &cost_guard,
            &txn,
        )
        .await
        .expect("label_one_transaction should resolve via the LLM tier and publish");
    }

    let normalized_key = webapp::labeling::normalize::normalize(Some("Fitness First"), None);
    let rule_id = webapp::kafka::labeling::learned_rule_uuid(&normalized_key, "personal.gym");
    let learned = proj::find_rule(&db, rule_id)
        .await
        .expect("query rule")
        .expect("three agreeing LLM observations must learn a rule");
    assert_eq!(learned.origin, webapp::kafka::labeling::RuleOrigin::Learned);
}

/// Uses WP2's real `normalize::normalize` to assert two differently-cased/
/// punctuated counterparty strings land on the same `counterparty_key` (so
/// they share rule/cache/learning observations).
#[tokio::test]
async fn differently_formatted_counterparties_share_a_normalized_key() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url())
        .await
        .expect("connect to test Postgres");
    let publisher =
        EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    seed_categories(&db, &["uncategorized", "personal.gym"]).await;
    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let provider = categorizer::provider::fake::FakeProvider::new();
    let ops = LabelingOps::real(3, 0.9);
    let cost_guard = CostGuard::new(10);

    let txn_a = seed_transaction(&db, "KEY-TEST-A", "FITNESS FIRST GMBH").await;
    let txn_b = seed_transaction(&db, "KEY-TEST-B", "fitness first //kartenzahlung").await;

    for txn in [&txn_a, &txn_b] {
        label_one_transaction(
            &db,
            &publisher,
            &ops,
            &provider,
            &catalog,
            "test-prompt-v1",
            0.0,
            &cost_guard,
            txn,
        )
        .await
        .expect("label_one_transaction should resolve via the LLM tier and publish");
    }

    let reloaded_a = proj::find_transaction(&db, txn_a.id)
        .await
        .expect("reload transaction a")
        .expect("transaction a still exists");
    let reloaded_b = proj::find_transaction(&db, txn_b.id)
        .await
        .expect("reload transaction b")
        .expect("transaction b still exists");

    assert!(reloaded_a.counterparty_key.is_some());
    assert_eq!(reloaded_a.counterparty_key, reloaded_b.counterparty_key);
}
