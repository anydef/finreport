//! `proposePurchaseOrderCategories`: the chain (rule, cache, LLM), the cache
//! write-back, the cost guard, and the guarantee that a proposal changes
//! nothing the user owns. Postgres only; the cache sink is a recorder and the
//! provider a counting fake, so no broker and no network are involved.
#![cfg(feature = "integration")]

mod common;

use async_graphql::Request;
use categorizer::provider::{
    CategoryCatalog, LabelProvider, LabelRequest, LabelSuggestion, ProviderError,
};
use chrono::{NaiveDate, Utc};
use entity::entities::{
    purchase_order, purchase_order_item, purchase_order_split, rule, transaction_label,
    transaction_split, transaction_user_label,
};
use rust_decimal::Decimal;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, Set};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use uuid::Uuid;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::purchase_order_proposals::{
    item_fingerprint, propose_for_items, CacheSink, ProposalContext, ProposalSource,
};
use webapp::graphql::purchase_orders::order_from_rows;
use webapp::graphql::{create_schema, request_with_auth, RawSessionToken};
use webapp::kafka::labeling::{category_uuid, CacheRecord, CURRENT_SCHEMA_VERSION};
use webapp::labeling::processor::CostGuard;
use webapp::projection::labeling as proj;

/// Answers every request with `slug` and counts the calls.
struct Counting {
    slug: Option<String>,
    calls: AtomicU32,
}

impl Counting {
    fn new(slug: Option<&str>) -> Self {
        Self { slug: slug.map(str::to_string), calls: AtomicU32::new(0) }
    }
    fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl LabelProvider for Counting {
    fn id(&self) -> &'static str {
        "counting"
    }
    fn model(&self) -> &str {
        "counting-v1"
    }
    async fn suggest(&self, _: &LabelRequest<'_>) -> Result<LabelSuggestion, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(LabelSuggestion {
            category_slug: self.slug.clone(),
            proposed_path: None,
            confidence: 0.8,
            ambiguous: false,
            reasoning: None,
        })
    }
}

/// Records what would go to the topic and projects it, as the real sink does.
struct Recorder<'a> {
    db: &'a DatabaseConnection,
    stored: Mutex<Vec<CacheRecord>>,
}

#[async_trait::async_trait]
impl CacheSink for Recorder<'_> {
    async fn store(&self, record: &CacheRecord) {
        self.stored.lock().unwrap().push(record.clone());
        proj::project_llm_cache(self.db, &record.fingerprint, Some(record.clone())).await.unwrap();
    }
}

struct Fixture {
    user: AuthenticatedUser,
    account: Uuid,
    order_id: String,
    /// Unique per run, so the shared database's other rules and cache rows
    /// never meet these titles.
    titles: Vec<String>,
    slug: String,
}

/// A user, an order of two items, and one catalog category to propose.
async fn fixture(db: &DatabaseConnection) -> Fixture {
    let n = Uuid::new_v4().simple().to_string();
    let (user_id, username) = common::seed_user(db, &format!("pp-{n}"), "pw").await;
    let account = common::seed_account(db, "EUR", &format!("pp-{n}")).await;
    common::link(db, user_id, account).await;
    let (_, slug) = common::seed_category(db, &format!("expense.pp{n}"), "PP", "EXPENSE").await;
    let order_id = format!("PP-{n}");
    let titles = vec![format!("Zqx{n} Ladegeraet"), format!("Zqy{n} Kabel")];
    purchase_order::ActiveModel {
        user_id: Set(user_id),
        source: Set("amazon".into()),
        order_id: Set(order_id.clone()),
        order_date: Set(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()),
        total: Set(Decimal::new(2259, 2)),
        currency: Set("EUR".into()),
        price_basis: Set("line_total".into()),
        discounts_netted: Set(false),
        remainder: Set(Decimal::ZERO),
        revision: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .unwrap();
    for (i, title) in titles.iter().enumerate() {
        purchase_order_item::ActiveModel {
            user_id: Set(user_id),
            source: Set("amazon".into()),
            order_id: Set(order_id.clone()),
            position: Set(i as i32),
            asin: Set(format!("B00{i}")),
            quantity: Set(1),
            price: Set(Decimal::new(1000, 2)),
            discount: Set(None),
            promotion: Set(None),
            promotion_amount: Set(None),
            title: Set(title.clone()),
            status: Set("Delivered".into()),
            returned: Set(false),
            line_total: Set(Decimal::new(1000, 2)),
        }
        .insert(db)
        .await
        .unwrap();
    }
    Fixture { user: AuthenticatedUser { user_id, username, account_ids: vec![account] }, account, order_id, titles, slug }
}

async fn order_of(db: &DatabaseConnection, f: &Fixture) -> amazon_orders::Order {
    let row = purchase_order::Entity::find()
        .filter(purchase_order::Column::UserId.eq(f.user.user_id))
        .filter(purchase_order::Column::OrderId.eq(f.order_id.clone()))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let items = purchase_order_item::Entity::find()
        .filter(purchase_order_item::Column::UserId.eq(f.user.user_id))
        .filter(purchase_order_item::Column::OrderId.eq(f.order_id.clone()))
        .all(db)
        .await
        .unwrap();
    order_from_rows(&row, &items)
}

/// A category with the id the projection gives it (`category_uuid(slug)`),
/// which is how rules and the cache refer to a category.
async fn projected_category(db: &DatabaseConnection, prefix: &str) -> (Uuid, String) {
    let slug = format!("{prefix}_{}", Uuid::new_v4().simple());
    let id = category_uuid(&slug);
    entity::entities::category::ActiveModel {
        id: Set(id),
        slug: Set(slug.clone()),
        parent_id: Set(None),
        name: Set("PP".into()),
        kind: Set("EXPENSE".into()),
        depth: Set(1),
        sort_order: Set(0),
        archived: Set(false),
        origin: Set("seed".into()),
        owner_user_id: Set(None),
        revision: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .unwrap();
    (id, slug)
}

async fn add_rule(db: &DatabaseConnection, contains: &str, category: Uuid) {
    let conditions = serde_json::json!({ "description_contains": contains });
    rule::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set("po proposal test".into()),
        category_id: Set(category),
        conditions: Set(conditions),
        priority: Set(0),
        state: Set("active".into()),
        origin: Set("user".into()),
        auto_approved: Set(false),
        user_touched: Set(true),
        confidence: Set(None),
        evidence: Set(None),
        created_at: Set(Utc::now().into()),
        revision: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn run(
    db: &DatabaseConnection,
    f: &Fixture,
    provider: &Counting,
    guard: &CostGuard,
    sink: &Recorder<'_>,
) -> Vec<webapp::graphql::purchase_order_proposals::PurchaseOrderItemProposal> {
    let rules = proj::active_rules(db).await.unwrap();
    let catalog: CategoryCatalog = proj::build_catalog(db).await.unwrap();
    let ctx = ProposalContext {
        rules: &rules,
        catalog: &catalog,
        provider: Some(provider),
        prompt_version: "2",
        cost_guard: guard,
        sink,
    };
    propose_for_items(db, &ctx, &order_of(db, f).await).await.unwrap()
}

fn recorder(db: &DatabaseConnection) -> Recorder<'_> {
    Recorder { db, stored: Mutex::new(vec![]) }
}

#[tokio::test]
async fn a_rule_hit_beats_a_cache_hit_and_never_reaches_the_provider() {
    let db = common::db().await;
    let f = fixture(&db).await;
    let (rule_cat, rule_slug) = projected_category(&db, "expense.ppr").await;
    add_rule(&db, &f.titles[0].to_lowercase(), rule_cat).await;

    // A cached answer for the same item, in a different category.
    let provider = Counting::new(Some(&f.slug));
    let order = order_of(&db, &f).await;
    let fp = item_fingerprint(&provider, "2", &order.items[0]);
    proj::project_llm_cache(
        &*db,
        &fp,
        Some(CacheRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            fingerprint: fp.clone(),
            category_slug: Some(f.slug.clone()),
            proposed_path: None,
            confidence: 0.9,
            provider: "counting".into(),
            model: "counting-v1".into(),
            prompt_version: "2".into(),
            reasoning: None,
            created_at: Utc::now(),
        }),
    )
    .await
    .unwrap();

    let sink = recorder(&db);
    let out = run(&db, &f, &provider, &CostGuard::new(10), &sink).await;
    assert_eq!(out[0].source, ProposalSource::Rule);
    assert_eq!(out[0].category_slug.as_deref(), Some(rule_slug.as_str()));
    assert_eq!(out[0].confidence, None);
    // The second item has neither rule nor cache: the provider answers it.
    assert_eq!(out[1].source, ProposalSource::Llm);
    assert_eq!(provider.calls(), 1);
}

#[tokio::test]
async fn a_cache_hit_avoids_the_provider_and_a_provider_answer_is_written_back_once() {
    let db = common::db().await;
    let f = fixture(&db).await;
    let provider = Counting::new(Some(&f.slug));
    let sink = recorder(&db);

    let first = run(&db, &f, &provider, &CostGuard::new(10), &sink).await;
    assert!(first.iter().all(|p| p.source == ProposalSource::Llm));
    assert_eq!(provider.calls(), 2);
    let stored = sink.stored.lock().unwrap().clone();
    assert_eq!(stored.len(), 2, "each fresh answer is written back");
    assert!(stored.iter().all(|r| r.category_slug.as_deref() == Some(f.slug.as_str())
        && r.provider == "counting" && r.prompt_version == "2"));
    let order = order_of(&db, &f).await;
    let fp = item_fingerprint(&provider, "2", &order.items[0]);
    assert!(stored.iter().any(|r| r.fingerprint == fp), "stored under the shared fingerprint");
    assert!(proj::find_cache(&*db, &fp).await.unwrap().is_some());

    // A second ask (think: a second order of the same product) costs nothing.
    let second = run(&db, &f, &provider, &CostGuard::new(10), &sink).await;
    assert!(second.iter().all(|p| p.source == ProposalSource::Cache));
    assert_eq!(second[0].category_slug.as_deref(), Some(f.slug.as_str()));
    assert!(second[0].confidence.is_some());
    assert_eq!(provider.calls(), 2, "no further provider call");
    assert_eq!(sink.stored.lock().unwrap().len(), 2, "nothing rewritten");
}

#[tokio::test]
async fn an_item_nobody_can_categorise_is_none_not_an_error_and_the_budget_is_honoured() {
    let db = common::db().await;
    let f = fixture(&db).await;

    // The provider declines: NONE, and the decline is cached (no re-pay).
    let decliner = Counting::new(None);
    let sink = recorder(&db);
    let out = run(&db, &f, &decliner, &CostGuard::new(10), &sink).await;
    assert!(out.iter().all(|p| p.source == ProposalSource::None && p.category_slug.is_none()));
    assert_eq!(out[0].reason.as_deref(), Some("no_category"));
    let again = run(&db, &f, &decliner, &CostGuard::new(10), &sink).await;
    assert_eq!(again[0].source, ProposalSource::None);
    assert_eq!(decliner.calls(), 2, "the second ask is served from the cache");

    // A budget of one call: the first item is asked, the second is refused
    // quietly and says why.
    let g = fixture(&db).await;
    let provider = Counting::new(Some(&g.slug));
    let out = run(&db, &g, &provider, &CostGuard::new(1), &recorder(&db)).await;
    assert_eq!(out[0].source, ProposalSource::Llm);
    assert_eq!(out[1].source, ProposalSource::None);
    assert_eq!(out[1].reason.as_deref(), Some("llm_budget_exhausted"));
    assert_eq!(provider.calls(), 1);
}

async fn user_rows(db: &DatabaseConnection, f: &Fixture, tx: Uuid) -> [u64; 4] {
    let _ = f;
    [
        transaction_split::Entity::find().filter(transaction_split::Column::TransactionId.eq(tx)).count(db).await.unwrap(),
        transaction_label::Entity::find().filter(transaction_label::Column::TransactionId.eq(tx)).count(db).await.unwrap(),
        transaction_user_label::Entity::find().filter(transaction_user_label::Column::TransactionId.eq(tx)).count(db).await.unwrap(),
        purchase_order_split::Entity::find().filter(purchase_order_split::Column::TransactionId.eq(tx)).count(db).await.unwrap(),
    ]
}

#[tokio::test]
async fn the_mutation_changes_no_transaction_split_or_label() {
    let db = common::db().await;
    let f = fixture(&db).await;
    let tx = common::seed_transaction(&db, f.account, "2026-10-08", "-22.59", Some("AMAZON PAYMENTS EUROPE S.C.A.")).await;
    let (cat, _) = common::seed_category(&db, "expense.ppl", "PPL", "EXPENSE").await;
    common::seed_transaction_label(&db, tx, Some(cat), "RULE", "RESOLVED").await;
    let tx_before = entity::entities::transaction::Entity::find_by_id(tx).one(&*db).await.unwrap().unwrap();
    let before = user_rows(&db, &f, tx).await;
    let splits_total = purchase_order_split::Entity::find().count(&*db).await.unwrap();
    let label_before = transaction_label::Entity::find_by_id(tx).one(&*db).await.unwrap().unwrap();

    // Through the real schema: no broker, the default `fake` provider.
    let schema = create_schema(db.clone(), common::dummy_settings());
    let q = format!(
        r#"mutation {{ proposePurchaseOrderCategories(orderId: "{}") {{ position asin title categorySlug source confidence reason }} }}"#,
        f.order_id
    );
    let r = schema
        .execute(request_with_auth(Request::new(q), Some(f.user.clone()), RawSessionToken::default()))
        .await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let items = r.data.into_json().unwrap()["proposePurchaseOrderCategories"].clone();
    assert_eq!(items.as_array().unwrap().len(), 2);
    assert_eq!(items[0]["position"], 0);
    assert_eq!(items[0]["source"], "LLM", "the fake provider answered");

    assert_eq!(user_rows(&db, &f, tx).await, before);
    assert_eq!(purchase_order_split::Entity::find().count(&*db).await.unwrap(), splits_total);
    assert_eq!(
        entity::entities::transaction::Entity::find_by_id(tx).one(&*db).await.unwrap().unwrap(),
        tx_before
    );
    assert_eq!(transaction_label::Entity::find_by_id(tx).one(&*db).await.unwrap().unwrap(), label_before);
}

#[tokio::test]
async fn another_users_order_is_not_found() {
    let db = common::db().await;
    let f = fixture(&db).await;
    let other = fixture(&db).await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    let q = format!(r#"mutation {{ proposePurchaseOrderCategories(orderId: "{}") {{ asin }} }}"#, f.order_id);
    let r = schema
        .execute(request_with_auth(Request::new(q), Some(other.user.clone()), RawSessionToken::default()))
        .await;
    assert_eq!(r.errors.len(), 1);
}

// ---------------------------------------------------------------------------
// The read path (`purchaseOrderMatches`) and the process-wide budget, through
// the real schema.
// ---------------------------------------------------------------------------

const ITEMS_QUERY: &str = "{ purchaseOrderMatches { unmatchedOrders { orderId items { title suggestedCategorySlug suggestionSource } } } }";

/// The suggestion fields of `f`'s order, as the page would see them.
async fn read_items(schema: &webapp::graphql::AppSchema, f: &Fixture) -> Vec<serde_json::Value> {
    let r = schema
        .execute(request_with_auth(Request::new(ITEMS_QUERY), Some(f.user.clone()), RawSessionToken::default()))
        .await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let data = r.data.into_json().unwrap();
    let orders = data["purchaseOrderMatches"]["unmatchedOrders"].as_array().unwrap().clone();
    let order = orders.iter().find(|o| o["orderId"] == f.order_id.as_str()).expect("order in report");
    order["items"].as_array().unwrap().clone()
}

/// Stores `slug` as the cached answer for `order.items[position]` under the
/// fingerprint the read path will look up (the default `fake` provider's).
async fn seed_cache(db: &DatabaseConnection, f: &Fixture, position: usize, slug: &str) {
    let settings = common::dummy_settings();
    let provider = categorizer::factory::build_provider(&settings).unwrap();
    let order = order_of(db, f).await;
    let fp = item_fingerprint(provider.as_ref(), &settings.prompt_version, &order.items[position]);
    proj::project_llm_cache(
        db,
        &fp,
        Some(CacheRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            fingerprint: fp.clone(),
            category_slug: Some(slug.to_string()),
            proposed_path: None,
            confidence: 0.9,
            provider: provider.id().into(),
            model: provider.model().into(),
            prompt_version: settings.prompt_version.clone(),
            reasoning: None,
            created_at: Utc::now(),
        }),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn the_read_path_serves_a_stored_answer_and_a_rule_beats_a_different_cached_one() {
    let db = common::db().await;
    let f = fixture(&db).await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    let (_, cached_slug) = projected_category(&db, "expense.ppc").await;
    let (rule_cat, rule_slug) = projected_category(&db, "expense.ppr").await;

    // Item 0: cache only. Item 1: a rule AND a different cached answer.
    seed_cache(&db, &f, 0, &cached_slug).await;
    seed_cache(&db, &f, 1, &cached_slug).await;
    add_rule(&db, &f.titles[1].to_lowercase(), rule_cat).await;

    let items = read_items(&schema, &f).await;
    let item = |t: &str| items.iter().find(|i| i["title"] == t).unwrap().clone();
    let from_cache = item(&f.titles[0]);
    assert_eq!(from_cache["suggestedCategorySlug"], cached_slug.as_str());
    assert_eq!(from_cache["suggestionSource"], "CACHE");
    let from_rule = item(&f.titles[1]);
    assert_eq!(from_rule["suggestedCategorySlug"], rule_slug.as_str());
    assert_eq!(from_rule["suggestionSource"], "RULE", "the rule outranks the cache");
}

#[tokio::test]
async fn an_item_with_neither_rule_nor_cache_reports_no_suggestion_and_the_query_succeeds() {
    let db = common::db().await;
    let f = fixture(&db).await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    let items = read_items(&schema, &f).await;
    assert_eq!(items.len(), 2);
    for i in &items {
        assert!(i["suggestedCategorySlug"].is_null(), "{i}");
        assert_eq!(i["suggestionSource"], "NONE");
    }
}

#[tokio::test]
async fn the_shared_budget_bounds_the_process_not_one_call() {
    let db = common::db().await;
    let mut settings = (*common::dummy_settings()).clone();
    settings.llm_max_requests_per_run = 1;
    settings.llm_budget_window_secs = 1_000_000; // no refill mid-test
    let schema = create_schema(db.clone(), std::sync::Arc::new(settings));

    let propose = |f: &Fixture| {
        let q = format!(
            r#"mutation {{ proposePurchaseOrderCategories(orderId: "{}") {{ source reason }} }}"#,
            f.order_id
        );
        request_with_auth(Request::new(q), Some(f.user.clone()), RawSessionToken::default())
    };
    let first = fixture(&db).await;
    let r = schema.execute(propose(&first)).await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let out = r.data.into_json().unwrap()["proposePurchaseOrderCategories"].clone();
    assert_eq!(out[0]["source"], "LLM", "the one allowance is spent here");
    assert_eq!(out[1]["reason"], "llm_budget_exhausted");

    // A separate call, with its own uncached items, finds the allowance gone.
    // Before the budget was process-wide, every call started with a fresh one.
    let second = fixture(&db).await;
    let r = schema.execute(propose(&second)).await;
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let out = r.data.into_json().unwrap()["proposePurchaseOrderCategories"].clone();
    for item in out.as_array().unwrap() {
        assert_eq!(item["source"], "NONE", "{item}");
        assert_eq!(item["reason"], "llm_budget_exhausted", "{item}");
    }
}
