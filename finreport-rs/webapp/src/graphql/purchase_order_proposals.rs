//! `proposePurchaseOrderCategories`: a *suggested* category per item of a
//! stored Amazon order, resolved through the same chain the labeler uses for
//! bank lines (rule, then LLM cache, then the LLM), with the item title in
//! place of the counterparty and description.
//!
//! **A proposal is only a suggestion.** Nothing here writes a split, a
//! label or a user override, and no transaction changes: the one write is
//! the LLM-cache entry for a fresh provider answer, which is a shared,
//! regenerable memo and not user data. Applying stays the explicit
//! `applyPurchaseOrderSplit` call.
//!
//! **One cache.** An item's fingerprint is built with
//! `labeling::fingerprint` from the configured provider, model and prompt
//! version, the normalised title as counterparty, an empty description and
//! `Debit`. A fresh answer is stored under it exactly as the labeler stores
//! its own (`CacheRecord`), so a second order of the same product costs no
//! call, and a rule or label the user later confirms still wins in front of
//! it. Cost is bounded by the labeler's `CostGuard`
//! (`APP_llm_max_requests_per_run`), one window per mutation call; a refused
//! item is `NONE` with reason `llm_budget_exhausted`, never an error.
//!
//! The decisions that need no I/O (`rule_hit`, `cache_hit`, `llm_answer`,
//! `item_fingerprint`) are pure and unit-tested; `propose_for_items` threads
//! them between the database, the provider and the cache sink.

use std::sync::Arc;

use async_graphql::{Enum, SimpleObject};
use categorizer::provider::{CategoryCatalog, LabelProvider, LabelRequest};
use chrono::Utc;
use entity::entities::llm_label_cache;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tracing::{debug, error, warn};
use uuid::Uuid;

use amazon_orders::{Item, Order};

use crate::graphql::current_user::AuthenticatedUser;
use crate::graphql::purchase_orders::{stored_orders, validation_error};
use crate::kafka::labeling::{
    category_uuid, CacheRecord, RuleRecord, CURRENT_SCHEMA_VERSION, ORIGIN_LABELER, TOPIC_LLM_CACHE,
};
use crate::kafka::producer::EventPublisher;
use crate::labeling::fingerprint::{fingerprint, Direction};
use crate::labeling::normalize::normalize;
use crate::labeling::processor::{cache_entry_is_usable, labeling_headers, validate_suggestion, CostGuard};
use crate::labeling::rules::{most_specific_match, RuleMatchInput};
use crate::projection::labeling as proj;

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ProposalSource {
    /// An active rule matched the item title.
    Rule,
    /// A stored LLM answer for this product, no call made.
    Cache,
    /// The configured provider answered just now.
    Llm,
    /// No proposal; `reason` says why.
    None,
}

#[derive(SimpleObject, Clone, Debug, PartialEq)]
pub struct PurchaseOrderItemProposal {
    /// Position of the item within the order (0-based). Unique, unlike `asin`.
    pub position: i32,
    pub asin: String,
    pub title: String,
    /// The suggested category, a slug the catalog contains. `null` for `NONE`.
    pub category_slug: Option<String>,
    pub source: ProposalSource,
    /// The model's confidence; `null` for a rule hit and for `NONE`.
    pub confidence: Option<f64>,
    /// Why there is no proposal (`NONE` only): `no_category`,
    /// `llm_budget_exhausted`, `provider_error` or `provider_unavailable`.
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Pure decisions
// ---------------------------------------------------------------------------

pub const REASON_NO_CATEGORY: &str = "no_category";
pub const REASON_BUDGET: &str = "llm_budget_exhausted";
pub const REASON_PROVIDER_ERROR: &str = "provider_error";
pub const REASON_PROVIDER_UNAVAILABLE: &str = "provider_unavailable";

/// What one step of the chain decided for one item.
#[derive(Debug, Clone, PartialEq)]
pub struct Decided {
    pub category_slug: Option<String>,
    pub source: ProposalSource,
    pub confidence: Option<f64>,
    pub reason: Option<&'static str>,
}

impl Decided {
    fn found(slug: String, source: ProposalSource, confidence: Option<f64>) -> Self {
        Self { category_slug: Some(slug), source, confidence, reason: None }
    }
    fn none(reason: &'static str) -> Self {
        Self { category_slug: None, source: ProposalSource::None, confidence: None, reason: Some(reason) }
    }
}

/// The catalog slug whose deterministic id is `id`.
fn slug_of(catalog: &CategoryCatalog, id: Uuid) -> Option<String> {
    catalog.entries.iter().find(|e| category_uuid(&e.slug) == id).map(|e| e.slug.clone())
}

/// The key an item is looked up by: the title, normalised like a counterparty.
pub fn item_key(item: &Item) -> String {
    normalize(Some(&item.title), None)
}

/// Step 1: the labeler's own matcher, over the item title. The title is the
/// description and, normalised, the counterparty key; an item is a debit of
/// its line total, and carries no IBAN, type or account.
pub fn rule_hit(rules: &[RuleRecord], catalog: &CategoryCatalog, item: &Item) -> Option<Decided> {
    let key = item_key(item);
    let input = RuleMatchInput {
        counterparty_key: Some(&key),
        counterparty_iban: None,
        description: Some(&item.title),
        transaction_type: None,
        direction: Some("SPENDING"),
        amount: -item.line_total.abs(),
        account_id: None,
    };
    let matched = most_specific_match(rules, &input)?;
    let id = proj::category_id_of(matched)?;
    Some(Decided::found(slug_of(catalog, id)?, ProposalSource::Rule, None))
}

/// The fingerprint under which an item's answer is cached; the labeler's
/// scheme, with no description (the title is the counterparty).
pub fn item_fingerprint(provider: &dyn LabelProvider, prompt_version: &str, item: &Item) -> String {
    fingerprint(provider.id(), provider.model(), prompt_version, &item_key(item), "", Direction::Debit)
}

/// Step 2: a usable stored answer. `Some(Decided)` ends the chain, including
/// a stored *decline* (no category), which must not be paid for again.
pub fn cache_hit(cached: &llm_label_cache::Model, catalog: &CategoryCatalog) -> Option<Decided> {
    if !cache_entry_is_usable(cached, catalog) {
        return None;
    }
    let Some(id) = cached.category_id else {
        return Some(Decided::none(REASON_NO_CATEGORY));
    };
    let confidence: f64 = cached.confidence.to_string().parse().unwrap_or(0.0);
    Some(Decided::found(slug_of(catalog, id)?, ProposalSource::Cache, Some(confidence)))
}

/// Step 3: a provider answer, validated against the catalog like the
/// labeler does. Returns the proposal and the cache record to store.
pub fn llm_answer(
    suggestion: categorizer::provider::LabelSuggestion,
    catalog: &CategoryCatalog,
    provider: &dyn LabelProvider,
    prompt_version: &str,
    fingerprint: &str,
) -> (Decided, CacheRecord) {
    let validated = validate_suggestion(suggestion, catalog);
    let s = validated.suggestion;
    let confidence = s.confidence.clamp(0.0, 1.0);
    let record = CacheRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        fingerprint: fingerprint.to_string(),
        category_slug: s.category_slug.clone(),
        proposed_path: s.proposed_path.clone(),
        confidence,
        provider: provider.id().to_string(),
        model: provider.model().to_string(),
        prompt_version: prompt_version.to_string(),
        reasoning: s.reasoning.clone(),
        created_at: Utc::now(),
    };
    let decided = match s.category_slug {
        Some(slug) => Decided::found(slug, ProposalSource::Llm, Some(f64::from(confidence))),
        None => Decided::none(REASON_NO_CATEGORY),
    };
    (decided, record)
}

// ---------------------------------------------------------------------------
// Edges
// ---------------------------------------------------------------------------

/// Where a fresh provider answer is stored.
#[async_trait::async_trait]
pub trait CacheSink: Send + Sync {
    async fn store(&self, record: &CacheRecord);
}

/// The real sink: publish to `finreport.llm-cache` (when a broker is
/// configured), then project, like every other dual-written record. Failure
/// to publish is logged and never fails the mutation: the answer is still
/// valid and the cache can be refilled.
pub struct KafkaCacheSink<'a> {
    pub db: &'a DatabaseConnection,
    pub publisher: Option<&'a Arc<EventPublisher>>,
}

#[async_trait::async_trait]
impl CacheSink for KafkaCacheSink<'_> {
    async fn store(&self, record: &CacheRecord) {
        match self.publisher {
            Some(publisher) => match serde_json::to_vec(record) {
                Ok(value) => {
                    if let Err(e) = publisher
                        .publish_with_headers(TOPIC_LLM_CACHE, &record.fingerprint, &value, labeling_headers(ORIGIN_LABELER))
                        .await
                    {
                        error!(error = %e, "purchase-order proposals: failed to publish llm-cache record");
                    }
                }
                Err(e) => error!(error = %e, "purchase-order proposals: failed to serialize llm-cache record"),
            },
            None => warn!("purchase-order proposals: no Kafka broker, llm-cache answer kept in Postgres only"),
        }
        if let Err(e) = proj::project_llm_cache(self.db, &record.fingerprint, Some(record.clone())).await {
            error!(error = %e, "purchase-order proposals: failed to project llm-cache record");
        }
    }
}

/// Everything the chain needs besides the items.
pub struct ProposalContext<'a> {
    pub rules: &'a [RuleRecord],
    pub catalog: &'a CategoryCatalog,
    /// `None` when the configured provider could not be built.
    pub provider: Option<&'a dyn LabelProvider>,
    pub prompt_version: &'a str,
    pub cost_guard: &'a CostGuard,
    pub sink: &'a dyn CacheSink,
}

/// Runs the chain for every item of `order`, in order, stopping each at the
/// first hit. Reads the cache through `db`; writes only through `ctx.sink`.
pub async fn propose_for_items(
    db: &impl ConnectionTrait,
    ctx: &ProposalContext<'_>,
    order: &Order,
) -> Result<Vec<PurchaseOrderItemProposal>, sea_orm::DbErr> {
    let mut out = Vec::with_capacity(order.items.len());
    for (position, item) in order.items.iter().enumerate() {
        let decided = decide_item(db, ctx, order, item).await?;
        out.push(PurchaseOrderItemProposal {
            position: position as i32,
            asin: item.asin.clone(),
            title: item.title.clone(),
            category_slug: decided.category_slug,
            source: decided.source,
            confidence: decided.confidence,
            reason: decided.reason.map(str::to_string),
        });
    }
    Ok(out)
}

async fn decide_item(
    db: &impl ConnectionTrait,
    ctx: &ProposalContext<'_>,
    order: &Order,
    item: &Item,
) -> Result<Decided, sea_orm::DbErr> {
    if let Some(hit) = rule_hit(ctx.rules, ctx.catalog, item) {
        return Ok(hit);
    }
    let Some(provider) = ctx.provider else {
        return Ok(Decided::none(REASON_PROVIDER_UNAVAILABLE));
    };
    let fp = item_fingerprint(provider, ctx.prompt_version, item);
    if let Some(cached) = proj::find_cache(db, &fp).await?
        && let Some(hit) = cache_hit(&cached, ctx.catalog)
    {
        return Ok(hit);
    }
    if !ctx.cost_guard.try_consume() {
        if ctx.cost_guard.note_denied() == 0 {
            warn!("purchase-order proposals: LLM budget exhausted (APP_llm_max_requests_per_run); remaining items get no proposal");
        }
        return Ok(Decided::none(REASON_BUDGET));
    }
    let key = item_key(item);
    let request = LabelRequest {
        counterparty: Some(&key),
        description: Some(&item.title),
        amount: -item.line_total.abs(),
        currency: &order.currency,
        booking_date: order.order_date,
        transaction_type: None,
        catalog: ctx.catalog,
        prompt_version: ctx.prompt_version,
    };
    let suggestion = match provider.suggest(&request).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "purchase-order proposals: provider error, no proposal for an item");
            return Ok(Decided::none(REASON_PROVIDER_ERROR));
        }
    };
    let (decided, record) = llm_answer(suggestion, ctx.catalog, provider, ctx.prompt_version, &fp);
    debug!(fingerprint = %fp, "purchase-order proposals: storing a fresh llm-cache answer");
    ctx.sink.store(&record).await;
    Ok(decided)
}

/// The mutation body: loads the caller's order, rules and catalog, builds the
/// configured provider, and resolves. Writes nothing but cache entries.
pub async fn propose_purchase_order_categories(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    settings: &utils::settings::Settings,
    user: &AuthenticatedUser,
    order_id: String,
) -> async_graphql::Result<Vec<PurchaseOrderItemProposal>> {
    let order = stored_orders(db, user.user_id, Some(std::slice::from_ref(&order_id)))
        .await?
        .into_iter()
        .find(|o| o.order_id == order_id)
        .ok_or_else(|| validation_error("no such order"))?;
    let rules = proj::active_rules(db).await?;
    let catalog = proj::build_catalog(db).await?;
    let provider = match categorizer::factory::build_provider(settings) {
        Ok(p) => Some(p),
        Err(e) => {
            warn!(error = %e, "purchase-order proposals: cannot build the LLM provider; only rules can answer");
            None
        }
    };
    let guard = CostGuard::new(settings.llm_max_requests_per_run);
    let sink = KafkaCacheSink { db, publisher };
    let ctx = ProposalContext {
        rules: &rules,
        catalog: &catalog,
        provider: provider.as_deref(),
        prompt_version: &settings.prompt_version,
        cost_guard: &guard,
        sink: &sink,
    };
    Ok(propose_for_items(db, &ctx, &order).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kafka::labeling::{RuleConditions, RuleOrigin, RuleState};
    use rust_decimal::Decimal;
    use categorizer::provider::{CatalogEntry, LabelSuggestion};
    use categorizer::provider::fake::FakeProvider;

    fn item(title: &str) -> Item {
        Item {
            asin: "B001".into(),
            quantity: 1,
            price: Decimal::new(1000, 2),
            discount: None,
            promotion: None,
            promotion_amount: None,
            title: title.into(),
            status: "Delivered".into(),
            returned: false,
            line_total: Decimal::new(1000, 2),
        }
    }

    fn catalog() -> CategoryCatalog {
        CategoryCatalog::new(vec![CatalogEntry {
            slug: "shopping.tech".into(),
            name: "Tech".into(),
            kind: "EXPENSE".into(),
        }])
    }

    fn rule(conditions: RuleConditions, slug: &str) -> RuleRecord {
        RuleRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: Uuid::from_bytes([7; 16]),
            name: "r".into(),
            category_slug: category_uuid(slug).to_string(),
            conditions,
            priority: 0,
            state: RuleState::Active,
            origin: RuleOrigin::User,
            auto_approved: false,
            user_touched: true,
            confidence: None,
            evidence: None,
            created_at: Utc::now(),
            revision: Utc::now(),
        }
    }

    #[test]
    fn a_rule_on_the_product_name_hits_through_the_labelers_matcher() {
        let by_text = RuleConditions { description_contains: Some("anker".into()), ..Default::default() };
        let hit = rule_hit(&[rule(by_text, "shopping.tech")], &catalog(), &item("Anker Ladegeraet 65W")).unwrap();
        assert_eq!(hit.source, ProposalSource::Rule);
        assert_eq!(hit.category_slug.as_deref(), Some("shopping.tech"));
        let by_key = RuleConditions { counterparty_key: Some("usb kabel".into()), ..Default::default() };
        assert!(rule_hit(&[rule(by_key, "shopping.tech")], &catalog(), &item("USB Kabel")).is_some());
    }

    #[test]
    fn no_matching_rule_or_an_unknown_category_is_no_hit() {
        let c = RuleConditions { description_contains: Some("anker".into()), ..Default::default() };
        assert!(rule_hit(&[rule(c.clone(), "shopping.tech")], &catalog(), &item("Kabel")).is_none());
        assert!(rule_hit(&[rule(c, "gone.category")], &catalog(), &item("Anker")).is_none());
    }

    #[test]
    fn the_fingerprint_ignores_price_and_case_and_follows_the_provider() {
        let p = FakeProvider::new();
        let mut dear = item("USB  Kabel");
        dear.line_total = Decimal::new(9999, 2);
        assert_eq!(item_fingerprint(&p, "2", &item("usb kabel")), item_fingerprint(&p, "2", &dear));
        assert_ne!(item_fingerprint(&p, "2", &item("usb kabel")), item_fingerprint(&p, "3", &item("usb kabel")));
    }

    #[test]
    fn a_provider_answer_is_validated_and_yields_the_record_to_cache() {
        let p = FakeProvider::new();
        let known = LabelSuggestion {
            category_slug: Some("shopping.tech".into()),
            proposed_path: None,
            confidence: 1.7,
            ambiguous: false,
            reasoning: None,
        };
        let (d, rec) = llm_answer(known, &catalog(), &p, "2", "fp");
        assert_eq!((d.source, d.confidence), (ProposalSource::Llm, Some(1.0)));
        assert_eq!((rec.fingerprint.as_str(), rec.category_slug.as_deref()), ("fp", Some("shopping.tech")));

        let invented = LabelSuggestion { category_slug: Some("made.up".into()), ..LabelSuggestion {
            category_slug: None, proposed_path: None, confidence: 0.9, ambiguous: false, reasoning: None } };
        let (d, rec) = llm_answer(invented, &catalog(), &p, "2", "fp");
        assert_eq!((d.source, d.reason), (ProposalSource::None, Some(REASON_NO_CATEGORY)));
        assert_eq!((rec.category_slug, rec.proposed_path.as_deref()), (None, Some("made.up")));
    }
}
