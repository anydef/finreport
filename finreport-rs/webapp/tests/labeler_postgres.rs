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
use categorizer::provider::LabelProvider as _;
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
        // Derived from the caller's (always-unique) external_id, not a
        // shared "ACC1" constant: tests that seed multiple transactions
        // (e.g. the learner's repeated-observation tests) call this helper
        // more than once and would otherwise collide on account's
        // (source, external_id) unique index.
        external_id: Set(format!("ACC-{external_id}")),
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
    let ops = LabelingOps::real(3, 1, 0.9);
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
    let ops = LabelingOps::real(3, 1, 0.9);
    let cost_guard = CostGuard::new(10);

    for i in 0..3 {
        let mut txn = seed_transaction(&db, &format!("LEARN-TEST-{i:02}"), "Fitness First").await;
        let normalized_key = webapp::labeling::normalize::normalize(txn.counterparty_name.as_deref(), None);
        proj::set_counterparty_key(&db, txn.id, &normalized_key)
            .await
            .expect("set counterparty key");
        txn.counterparty_key = Some(normalized_key);
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
    let ops = LabelingOps::real(3, 1, 0.9);
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

/// The LLM budget is per sweep, not per process: a sweep that spends it
/// leaves the rest unlabelled (no row, never a wrong one), and the next
/// sweep starts with a full budget and makes progress on the backlog.
#[tokio::test]
async fn each_sweep_gets_a_fresh_llm_budget_and_exhaustion_leaves_rows_unlabelled() {
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
    let ops = LabelingOps::real(3, 1, 0.9);
    let guard = CostGuard::new(1);

    for (i, name) in ["Fitness First", "Lidl Sagt Danke", "Netflix International"].iter().enumerate() {
        seed_transaction(&db, &format!("BUDGET-{i}"), name).await;
    }

    for expected_labelled in [1usize, 2, 3] {
        run_sweep(&db, &publisher, &ops, &provider, &catalog, "test-prompt-v1", 0.0, &guard, 10)
            .await
            .expect("sweep");
        let labelled = transaction_label::Entity::find().all(&db).await.expect("labels").len();
        assert_eq!(
            labelled, expected_labelled,
            "a budget of 1 labels exactly one more transaction per sweep"
        );
    }
}

// ---------------------------------------------------------------------------
// A human decision propagates to similar transactions
// ---------------------------------------------------------------------------

struct PanicProvider;
#[async_trait::async_trait]
impl categorizer::provider::LabelProvider for PanicProvider {
    fn id(&self) -> &'static str {
        "fake"
    }
    fn model(&self) -> &str {
        "fake-v1"
    }
    async fn suggest(
        &self,
        _req: &categorizer::provider::LabelRequest<'_>,
    ) -> Result<categorizer::provider::LabelSuggestion, categorizer::provider::ProviderError> {
        panic!("a sibling must be served by the rule, never by a fresh LLM call");
    }
}

struct CountingProvider(std::sync::atomic::AtomicU32);
#[async_trait::async_trait]
impl categorizer::provider::LabelProvider for CountingProvider {
    fn id(&self) -> &'static str {
        "fake"
    }
    fn model(&self) -> &str {
        "fake-v1"
    }
    async fn suggest(
        &self,
        _req: &categorizer::provider::LabelRequest<'_>,
    ) -> Result<categorizer::provider::LabelSuggestion, categorizer::provider::ProviderError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(categorizer::provider::LabelSuggestion {
            category_slug: Some("personal.gym".to_string()),
            proposed_path: None,
            confidence: 0.9,
            ambiguous: false,
            reasoning: None,
        })
    }
}

/// Seeds a "Fitness First" transaction with its `counterparty_key` already set.
async fn seed_fitness_first(db: &sea_orm::DatabaseConnection, external_id: &str) -> TransactionForLabeling {
    let mut txn = seed_transaction(db, external_id, "Fitness First").await;
    let key = webapp::labeling::normalize::normalize(txn.counterparty_name.as_deref(), None);
    proj::set_counterparty_key(db, txn.id, &key).await.expect("set counterparty key");
    txn.counterparty_key = Some(key);
    txn
}

fn user_override(txn: &TransactionForLabeling, slug: &str) -> webapp::kafka::labeling::UserLabelRecord {
    webapp::kafka::labeling::UserLabelRecord {
        schema_version: webapp::kafka::labeling::CURRENT_SCHEMA_VERSION,
        source: txn.source.clone(),
        external_id: txn.external_id.clone(),
        category_slug: Some(slug.to_string()),
        parts: Vec::new(),
        tags: Vec::new(),
        recurring: None,
        revision: Utc::now(),
        note: None,
    }
}

/// The fingerprint the labeler computes for a "Fitness First" debit under the
/// `FakeProvider` and `test-prompt-v1`.
fn fitness_first_fingerprint() -> String {
    webapp::labeling::fingerprint::fingerprint(
        "fake",
        categorizer::provider::fake::FakeProvider::new().model(),
        "test-prompt-v1",
        &webapp::labeling::normalize::normalize(Some("Fitness First"), None),
        &webapp::labeling::normalize::normalize(None, None),
        webapp::labeling::fingerprint::Direction::Debit,
    )
}

/// The whole point: the user corrects one transaction; the sibling that
/// previously would have been served the LLM's rejected answer from the cache
/// now resolves, through the real chain and with no LLM call, to the user's
/// category.
#[tokio::test]
async fn a_user_override_makes_a_sibling_resolve_to_the_users_category() {
    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url()).await.expect("connect to test Postgres");
    let publisher = EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    seed_categories(&db, &["uncategorized", "personal.gym", "food.restaurants"]).await;
    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let provider = categorizer::provider::fake::FakeProvider::new();
    let ops = LabelingOps::real(3, 1, 0.9);
    let cost_guard = CostGuard::new(10);
    let fp = fitness_first_fingerprint();

    // 1. The LLM labels the first transaction "personal.gym" and caches it.
    let corrected = seed_fitness_first(&db, "PROPAGATE-A").await;
    label_one_transaction(&db, &publisher, &ops, &provider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &corrected)
        .await
        .expect("llm label");
    let cached = proj::find_cache(&db, &fp).await.expect("find cache").expect("the LLM answer is cached");
    assert_eq!(cached.category_id, Some(category_uuid("personal.gym")));
    assert!(proj::find_rule(&db, webapp::kafka::labeling::learned_rule_uuid(
        &webapp::labeling::normalize::normalize(Some("Fitness First"), None), "personal.gym")).await.unwrap().is_none(),
        "one LLM observation must not learn a rule");

    // 2. The human says it is a restaurant.
    proj::project_user_label(&db, corrected.id, Some(user_override(&corrected, "food.restaurants")))
        .await
        .expect("project user label");
    label_one_transaction(&db, &publisher, &ops, &provider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &corrected)
        .await
        .expect("user override resolves");

    assert!(
        proj::find_cache(&db, &fp).await.expect("find cache").is_none(),
        "the contradicted cache entry must be dropped"
    );
    let key = webapp::labeling::normalize::normalize(Some("Fitness First"), None);
    let rule = proj::find_rule(&db, webapp::kafka::labeling::learned_rule_uuid(&key, "food.restaurants"))
        .await
        .expect("find rule")
        .expect("one user decision learns a rule");
    assert_eq!(rule.state, webapp::kafka::labeling::RuleState::Active, "confidence 1.0 auto-approves");

    // 3. A sibling resolves to the user's category; the provider would panic.
    let sibling = seed_fitness_first(&db, "PROPAGATE-B").await;
    label_one_transaction(&db, &publisher, &ops, &PanicProvider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &sibling)
        .await
        .expect("sibling resolves via the rule");
    let label = transaction_label::Entity::find_by_id(sibling.id).one(&db).await.unwrap().expect("sibling label");
    assert_eq!(label.label_source, "rule");
    assert_eq!(label.category_id, Some(category_uuid("food.restaurants")));
    assert_eq!(label.rule_id, Some(rule.id));
}

/// With no rule able to form, the invalidation alone still stops the stale
/// answer being served from the cache: the sibling is re-asked (one provider
/// call), not handed the rejected entry.
#[tokio::test]
async fn a_user_override_drops_the_stale_cache_entry_even_without_a_rule() {
    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url()).await.expect("connect to test Postgres");
    let publisher = EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    seed_categories(&db, &["uncategorized", "personal.gym", "food.restaurants"]).await;
    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let provider = categorizer::provider::fake::FakeProvider::new();
    // A learner that never fires isolates Change 2 from Change 1.
    let ops = LabelingOps::real(u32::MAX, u32::MAX, 0.9);
    let cost_guard = CostGuard::new(10);
    let fp = fitness_first_fingerprint();

    let corrected = seed_fitness_first(&db, "NORULE-A").await;
    label_one_transaction(&db, &publisher, &ops, &provider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &corrected)
        .await
        .expect("llm label");
    assert!(proj::find_cache(&db, &fp).await.unwrap().is_some());

    proj::project_user_label(&db, corrected.id, Some(user_override(&corrected, "food.restaurants")))
        .await
        .unwrap();
    label_one_transaction(&db, &publisher, &ops, &provider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &corrected)
        .await
        .unwrap();
    assert!(proj::find_cache(&db, &fp).await.unwrap().is_none(), "stale entry dropped");

    // The sibling is no longer served from the cache: it costs a provider call.
    let counting = CountingProvider(std::sync::atomic::AtomicU32::new(0));
    let sibling = seed_fitness_first(&db, "NORULE-B").await;
    label_one_transaction(&db, &publisher, &ops, &counting, &catalog, "test-prompt-v1", 0.0, &cost_guard, &sibling)
        .await
        .expect("sibling resolves");
    assert_eq!(counting.0.load(std::sync::atomic::Ordering::SeqCst), 1, "re-asked, not served the stale entry");
}

/// An entry that agrees with the human is still correct and stays cached.
#[tokio::test]
async fn a_user_override_that_agrees_with_the_cache_keeps_it() {
    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url()).await.expect("connect to test Postgres");
    let publisher = EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    seed_categories(&db, &["uncategorized", "personal.gym"]).await;
    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let provider = categorizer::provider::fake::FakeProvider::new();
    let ops = LabelingOps::real(u32::MAX, u32::MAX, 0.9);
    let cost_guard = CostGuard::new(10);

    let txn = seed_fitness_first(&db, "AGREE-A").await;
    label_one_transaction(&db, &publisher, &ops, &provider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &txn)
        .await
        .unwrap();
    proj::project_user_label(&db, txn.id, Some(user_override(&txn, "personal.gym"))).await.unwrap();
    label_one_transaction(&db, &publisher, &ops, &provider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &txn)
        .await
        .unwrap();
    assert!(proj::find_cache(&db, &fitness_first_fingerprint()).await.unwrap().is_some());
}

/// Two humans putting one counterparty in two categories is not a rule, even
/// though each decision alone would be enough at the default threshold of 1.
/// (`min_user_observations = 2` here so the first decision does not already
/// publish a rule before the conflict exists; the pure conflict rule at the
/// default is covered in `labeling::learn`'s unit tests.)
#[tokio::test]
async fn conflicting_user_decisions_learn_no_rule() {
    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url()).await.expect("connect to test Postgres");
    let publisher = EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    seed_categories(&db, &["uncategorized", "personal.gym", "food.restaurants"]).await;
    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let provider = categorizer::provider::fake::FakeProvider::new();
    let ops = LabelingOps::real(3, 2, 0.9);
    let cost_guard = CostGuard::new(10);

    for (id, slug) in [("CONFLICT-A", "food.restaurants"), ("CONFLICT-B", "personal.gym")] {
        let txn = seed_fitness_first(&db, id).await;
        proj::project_user_label(&db, txn.id, Some(user_override(&txn, slug))).await.unwrap();
        label_one_transaction(&db, &publisher, &ops, &provider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &txn)
            .await
            .unwrap();
    }
    let key = webapp::labeling::normalize::normalize(Some("Fitness First"), None);
    for slug in ["food.restaurants", "personal.gym"] {
        assert!(
            proj::find_rule(&db, webapp::kafka::labeling::learned_rule_uuid(&key, slug)).await.unwrap().is_none(),
            "no rule for {slug}"
        );
    }
}

// ---------------------------------------------------------------------------
// Learning exemptions: "never learn a rule for this merchant"
// ---------------------------------------------------------------------------

fn fitness_first_key() -> String {
    webapp::labeling::normalize::normalize(Some("Fitness First"), None)
}

fn exemption_record(key: &str) -> webapp::kafka::labeling::LearningExemptionRecord {
    webapp::kafka::labeling::LearningExemptionRecord {
        schema_version: webapp::kafka::labeling::CURRENT_SCHEMA_VERSION,
        counterparty_key: key.to_string(),
        decided_by: None,
        revision: Utc::now(),
    }
}

async fn rule_rows(db: &sea_orm::DatabaseConnection) -> Vec<entity::entities::rule::Model> {
    entity::entities::rule::Entity::find().all(db).await.expect("list rules")
}

/// A user decision at confidence 1.0 would auto-approve a rule at the default
/// threshold; for an exempt merchant it produces no rule at all - not even an
/// `in_review` one - and a sibling is labelled by the LLM tier, not a rule.
#[tokio::test]
async fn an_exempt_merchant_learns_no_rule_even_from_a_user_decision() {
    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url()).await.expect("connect to test Postgres");
    let publisher = EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    seed_categories(&db, &["uncategorized", "personal.gym", "food.restaurants"]).await;
    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let provider = categorizer::provider::fake::FakeProvider::new();
    let ops = LabelingOps::real(3, 1, 0.9);
    let cost_guard = CostGuard::new(10);

    proj::project_learning_exemption(&db, &fitness_first_key(), Some(exemption_record(&fitness_first_key())))
        .await
        .expect("project exemption");

    let corrected = seed_fitness_first(&db, "EXEMPT-A").await;
    proj::project_user_label(&db, corrected.id, Some(user_override(&corrected, "food.restaurants")))
        .await
        .unwrap();
    label_one_transaction(&db, &publisher, &ops, &provider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &corrected)
        .await
        .expect("user override resolves");
    let own = transaction_label::Entity::find_by_id(corrected.id).one(&db).await.unwrap().expect("label");
    assert_eq!(own.label_source, "user", "the decision itself still applies");
    assert!(rule_rows(&db).await.is_empty(), "no rule, not even in_review");

    // The sibling is left to the rest of the chain (here the fake LLM), not
    // silently rule-labelled with the user's category.
    let sibling = seed_fitness_first(&db, "EXEMPT-B").await;
    label_one_transaction(&db, &publisher, &ops, &provider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &sibling)
        .await
        .expect("sibling resolves");
    let label = transaction_label::Entity::find_by_id(sibling.id).one(&db).await.unwrap().expect("sibling label");
    assert_ne!(label.label_source, "rule");
    assert_eq!(label.rule_id, None);
    assert!(rule_rows(&db).await.is_empty());
}

/// Exempting discards the merchant's untouched learned rule (its labels fall
/// back down the chain) but never a rule the user has touched.
#[tokio::test]
async fn exempting_discards_an_untouched_learned_rule_but_not_a_user_touched_one() {
    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url()).await.expect("connect to test Postgres");
    let publisher = std::sync::Arc::new(
        EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer"),
    );

    seed_categories(&db, &["uncategorized", "personal.gym", "food.restaurants"]).await;
    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let provider = categorizer::provider::fake::FakeProvider::new();
    let ops = LabelingOps::real(3, 1, 0.9);
    let cost_guard = CostGuard::new(10);
    let key = fitness_first_key();

    // A learned, active, untouched rule via one user decision; a sibling
    // picks it up.
    let corrected = seed_fitness_first(&db, "REVOKE-A").await;
    proj::project_user_label(&db, corrected.id, Some(user_override(&corrected, "food.restaurants")))
        .await
        .unwrap();
    label_one_transaction(&db, &publisher, &ops, &provider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &corrected)
        .await
        .unwrap();
    let rule_id = webapp::kafka::labeling::learned_rule_uuid(&key, "food.restaurants");
    assert!(proj::find_rule(&db, rule_id).await.unwrap().is_some(), "rule learned");
    let sibling = seed_fitness_first(&db, "REVOKE-B").await;
    label_one_transaction(&db, &publisher, &ops, &PanicProvider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &sibling)
        .await
        .unwrap();
    let before = transaction_label::Entity::find_by_id(sibling.id).one(&db).await.unwrap().unwrap();
    assert_eq!(before.label_source, "rule");

    // A user-touched learned-looking rule for the same merchant (another
    // category), standing in for one the user approved or edited.
    let touched_id = webapp::kafka::labeling::learned_rule_uuid(&key, "personal.gym");
    let mut touched = proj::find_rule(&db, rule_id).await.unwrap().unwrap();
    touched.id = touched_id;
    touched.category_slug = "personal.gym".to_string();
    touched.user_touched = true;
    proj::project_rule(&db, touched_id, Some(touched)).await.unwrap();

    webapp::graphql::learning_exemptions::exempt_from_learning(&db, Some(&publisher), &[], Uuid::new_v4(), &key)
        .await
        .expect("exempt");

    assert!(proj::is_learning_exempt(&db, &key).await.unwrap());
    assert!(proj::find_rule(&db, rule_id).await.unwrap().is_none(), "untouched learned rule discarded");
    assert!(proj::find_rule(&db, touched_id).await.unwrap().is_some(), "user-touched rule kept");

    // The discarded rule no longer labels anything: remove the kept rule from
    // play so the sibling genuinely falls through to the next source.
    entity::entities::rule::Entity::delete_by_id(touched_id).exec(&db).await.unwrap();
    let after_provider = CountingProvider(std::sync::atomic::AtomicU32::new(0));
    label_one_transaction(&db, &publisher, &ops, &after_provider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &sibling)
        .await
        .unwrap();
    let after = transaction_label::Entity::find_by_id(sibling.id).one(&db).await.unwrap().unwrap();
    assert_ne!(after.label_source, "rule");
    assert_eq!(after.rule_id, None);
    assert!(rule_rows(&db).await.is_empty(), "nothing relearned while exempt");
}

/// Lifting the exemption lets the learner work again.
#[tokio::test]
async fn un_exempting_restores_normal_learning() {
    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url()).await.expect("connect to test Postgres");
    let publisher = std::sync::Arc::new(
        EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer"),
    );

    seed_categories(&db, &["uncategorized", "personal.gym", "food.restaurants"]).await;
    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let provider = categorizer::provider::fake::FakeProvider::new();
    let ops = LabelingOps::real(3, 1, 0.9);
    let cost_guard = CostGuard::new(10);
    let key = fitness_first_key();

    webapp::graphql::learning_exemptions::exempt_from_learning(&db, Some(&publisher), &[], Uuid::new_v4(), &key)
        .await
        .expect("exempt");
    let existed = webapp::graphql::learning_exemptions::remove_learning_exemption(&db, Some(&publisher), &key)
        .await
        .expect("un-exempt");
    assert!(existed);
    assert!(!proj::is_learning_exempt(&db, &key).await.unwrap());

    let corrected = seed_fitness_first(&db, "RESTORE-A").await;
    proj::project_user_label(&db, corrected.id, Some(user_override(&corrected, "food.restaurants")))
        .await
        .unwrap();
    label_one_transaction(&db, &publisher, &ops, &provider, &catalog, "test-prompt-v1", 0.0, &cost_guard, &corrected)
        .await
        .unwrap();
    let rule = proj::find_rule(&db, webapp::kafka::labeling::learned_rule_uuid(&key, "food.restaurants"))
        .await
        .unwrap()
        .expect("learning works again");
    assert_eq!(rule.state, webapp::kafka::labeling::RuleState::Active);
}

type ExemptionLog = Vec<(&'static str, Option<webapp::kafka::labeling::LearningExemptionRecord>)>;

async fn apply_exemption_log(db: &sea_orm::DatabaseConnection, log: &ExemptionLog) -> Vec<(String, Option<Uuid>)> {
    for (key, record) in log {
        proj::project_learning_exemption(db, key, record.clone()).await.unwrap();
    }
    let mut rows: Vec<_> = entity::entities::learning_exemption::Entity::find()
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.counterparty_key, r.decided_by))
        .collect();
    rows.sort();
    rows
}

/// Projection: a record projects, a stale one is ignored, a tombstone removes
/// it, and replaying the same log from offset 0 into an empty table gives
/// back the same rows.
#[tokio::test]
async fn learning_exemption_projects_tombstones_and_replays() {
    let pg = TestPostgres::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url()).await.expect("connect to test Postgres");

    let t0 = Utc::now();
    let log: ExemptionLog = vec![
        ("amazon", Some(webapp::kafka::labeling::LearningExemptionRecord { revision: t0, ..exemption_record("amazon") })),
        ("paypal", Some(webapp::kafka::labeling::LearningExemptionRecord { revision: t0, ..exemption_record("paypal") })),
        // Stale echo of amazon, older than what is stored: ignored.
        ("amazon", Some(webapp::kafka::labeling::LearningExemptionRecord {
            revision: t0 - chrono::Duration::seconds(60),
            decided_by: Some(Uuid::new_v4()),
            ..exemption_record("amazon")
        })),
        ("paypal", None),
    ];

    let first = apply_exemption_log(&db, &log).await;
    assert_eq!(first, vec![("amazon".to_string(), None)], "stale ignored, tombstone removed paypal");

    entity::entities::learning_exemption::Entity::delete_many().exec(&db).await.unwrap();
    assert_eq!(apply_exemption_log(&db, &log).await, first, "replay from offset 0 reproduces the table");
}

// ---------------------------------------------------------------------------
// Description-qualified learning: ambiguous merchants (PayPal) learn narrowly
// ---------------------------------------------------------------------------

/// A PayPal transaction whose raw description is `description`, key already set.
async fn seed_paypal(db: &sea_orm::DatabaseConnection, external_id: &str, description: &str) -> TransactionForLabeling {
    use sea_orm::IntoActiveModel;
    let mut txn = seed_transaction(db, external_id, "PayPal").await;
    let key = webapp::labeling::normalize::normalize(txn.counterparty_name.as_deref(), None);
    proj::set_counterparty_key(db, txn.id, &key).await.expect("set counterparty key");
    let row = transaction::Entity::find_by_id(txn.id).one(db).await.unwrap().expect("seeded row");
    let mut active = row.into_active_model();
    active.description = Set(Some(description.to_string()));
    active.update(db).await.expect("set description");
    txn.counterparty_key = Some(key);
    txn.description = Some(description.to_string());
    txn
}

const PAYPAL_NETFLIX: &str = "PayPal . NETFLIX INTERNATIONAL B.V. 1234567890 PP.1234.PP . , Ihr Einkauf bei NETFLIX";
const PAYPAL_ETSY: &str = "PayPal . ETSY IRELAND UC 5550001112 PP.5550.PP . , Ihr Einkauf bei ETSY";

/// Interleaved so no category has enough evidence (2 user decisions) before
/// the merchant is already known to be ambiguous - no broad rule sneaks in.
async fn decide_paypal_by_hand(
    db: &sea_orm::DatabaseConnection,
    publisher: &EventPublisher,
    ops: &LabelingOps,
    catalog: &categorizer::provider::CategoryCatalog,
) {
    let provider = categorizer::provider::fake::FakeProvider::new();
    let cost_guard = CostGuard::new(10);
    for (id, description, slug) in [
        ("PP-N1", PAYPAL_NETFLIX, "personal.gym"),
        ("PP-E1", PAYPAL_ETSY, "food.restaurants"),
        ("PP-N2", "PayPal . NETFLIX INTERNATIONAL B.V. 7770001 PP.7770.PP . , Ihr Einkauf bei NETFLIX", "personal.gym"),
        ("PP-E2", "PayPal . ETSY IRELAND UC 8880002 PP.8880.PP . , Ihr Einkauf bei ETSY", "food.restaurants"),
    ] {
        let txn = seed_paypal(db, id, description).await;
        proj::project_user_label(db, txn.id, Some(user_override(&txn, slug))).await.unwrap();
        label_one_transaction(db, publisher, ops, &provider, catalog, "test-prompt-v1", 0.0, &cost_guard, &txn)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn paypal_decisions_learn_narrow_rules_that_resolve_a_new_transaction_without_the_llm() {
    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url()).await.expect("connect to test Postgres");
    let publisher = EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    seed_categories(&db, &["uncategorized", "personal.gym", "food.restaurants"]).await;
    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let ops = LabelingOps::real(3, 2, 0.9);

    decide_paypal_by_hand(&db, &publisher, &ops, &catalog).await;

    let rules = rule_rows(&db).await;
    assert_eq!(rules.len(), 2, "one narrow rule per category, no merchant-wide one: {rules:?}");
    let mut contains: Vec<(String, Uuid)> = rules
        .iter()
        .map(|r| {
            let conditions: webapp::kafka::labeling::RuleConditions =
                serde_json::from_value(r.conditions.clone()).expect("conditions json");
            assert_eq!(conditions.counterparty_key.as_deref(), Some("paypal"));
            (conditions.description_contains.expect("narrow"), r.category_id)
        })
        .collect();
    contains.sort();
    assert_eq!(
        contains,
        vec![
            ("etsy".to_string(), category_uuid("food.restaurants")),
            ("netflix".to_string(), category_uuid("personal.gym"))
        ]
    );
    let netflix_rule_id = webapp::kafka::labeling::learned_narrow_rule_uuid("paypal", "personal.gym", "netflix");

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
            panic!("a narrow rule must short-circuit before ever calling the provider");
        }
    }

    let third = seed_paypal(
        &db,
        "PP-N3",
        "PayPal . NETFLIX INTERNATIONAL B.V. 4242424242 PP.4242.PP . , Ihr Einkauf bei NETFLIX",
    )
    .await;
    label_one_transaction(&db, &publisher, &ops, &PanicProvider, &catalog, "test-prompt-v1", 0.0, &CostGuard::new(10), &third)
        .await
        .expect("resolves from the narrow rule");
    let label = transaction_label::Entity::find_by_id(third.id).one(&db).await.unwrap().expect("label");
    assert_eq!(label.label_source, "rule");
    assert_eq!(label.rule_id, Some(netflix_rule_id));
}

#[tokio::test]
async fn an_exempt_paypal_learns_no_narrow_rule_either() {
    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url()).await.expect("connect to test Postgres");
    let publisher = EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    seed_categories(&db, &["uncategorized", "personal.gym", "food.restaurants"]).await;
    let catalog = proj::build_catalog(&db).await.expect("build catalog");
    let ops = LabelingOps::real(3, 2, 0.9);
    proj::project_learning_exemption(&db, "paypal", Some(exemption_record("paypal"))).await.expect("exempt");

    decide_paypal_by_hand(&db, &publisher, &ops, &catalog).await;
    assert!(rule_rows(&db).await.is_empty(), "exempt merchant: no rule of any kind");
}
