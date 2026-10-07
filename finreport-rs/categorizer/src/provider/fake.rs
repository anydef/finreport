//! The §2.9 `fake` provider: deterministic, offline, no API key. It is the
//! default (`APP_llm_provider=fake`), so nothing in local dev or CI ever
//! calls a paid API without being told to.
//!
//! Same input ⇒ same output, forever: known counterparty keys hit a
//! committed keyword table; anything else hashes to a stable slug at a fixed
//! below-threshold confidence. Two keys are wired to exercise the other two
//! review reasons (`ambiguous`, `new_category`) so the local demo and the
//! integration tests can reach every path with no network.

use super::{CategoryCatalog, LabelProvider, LabelRequest, LabelSuggestion, ProviderError};
use sha2::{Digest, Sha256};

/// `counterparty_key` (already normalized by `labeling::normalize`, §2.5) ⇒
/// `(category_slug, confidence)`. Committed so fixtures and tests can assert
/// on exact confidences; adding an entry is backward compatible, changing one
/// is not (it would relabel every transaction that has ever matched it).
const KEYWORD_TABLE: &[(&str, &str, f32)] = &[
    ("rewe", "food.groceries", 0.97),
    ("edeka", "food.groceries", 0.95),
    ("lidl", "food.groceries", 0.95),
    ("shell", "transportation.gas", 0.93),
    ("aral", "transportation.gas", 0.93),
    ("netflix", "entertainment.subscriptions", 0.9),
    ("spotify", "entertainment.subscriptions", 0.9),
    // Repeated merchant intended to cross `APP_rule_learn_min_observations`
    // (default 3) in the demo fixtures once this answer repeats.
    ("fitness first", "personal.gym", 0.88),
];

/// Keys that always resolve `ambiguous` regardless of confidence, to
/// exercise `review_reason=ambiguous` without depending on hash luck.
const AMBIGUOUS_KEYS: &[&str] = &["amazon"];

/// Keys that always propose a brand-new (never-auto-created) category path,
/// to exercise `review_reason=new_category`.
const PROPOSED_PATH_KEYS: &[(&str, &str)] = &[("acme co-working", "housing.coworking")];

/// Confidence assigned to an unrecognized key's deterministic fallback slug —
/// fixed below `APP_llm_min_confidence`'s default (0.5) so it always lands in
/// the review queue instead of silently mislabeling an unseen merchant.
const FALLBACK_CONFIDENCE: f32 = 0.42;

pub struct FakeProvider {
    model: &'static str,
}

impl FakeProvider {
    pub fn new() -> Self {
        Self { model: "fake-v1" }
    }
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Hashes `key` to a stable index into `catalog`, so an unrecognized
/// counterparty always gets the same fallback slug for the lifetime of the
/// catalog's entry order (the catalog is seeded from the committed
/// `prompts/taxonomy.json`, so this is stable in practice).
fn hash_to_catalog_slug(key: &str, catalog: &CategoryCatalog) -> Option<String> {
    if catalog.entries.is_empty() {
        return None;
    }
    let digest = Sha256::digest(key.as_bytes());
    // Fold the digest into a usize without relying on a particular integer
    // width; only determinism and uniform-enough spread matter here.
    let index = digest.iter().fold(0usize, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(*byte as usize)
    }) % catalog.entries.len();
    Some(catalog.entries[index].slug.clone())
}

#[async_trait::async_trait]
impl LabelProvider for FakeProvider {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn model(&self) -> &str {
        self.model
    }

    async fn suggest(&self, req: &LabelRequest<'_>) -> Result<LabelSuggestion, ProviderError> {
        // The labeler passes the already-normalized counterparty (or, when
        // absent, the normalized description — §2.5's fallback) as the key
        // this table is keyed against.
        let key = req
            .counterparty
            .filter(|value| !value.is_empty())
            .or(req.description)
            .unwrap_or_default();

        if let Some((_, slug)) = PROPOSED_PATH_KEYS.iter().find(|(k, _)| *k == key) {
            return Ok(LabelSuggestion {
                category_slug: None,
                proposed_path: Some((*slug).to_string()),
                confidence: 0.8,
                ambiguous: false,
                reasoning: Some(format!("fake provider: proposing new category {slug}")),
            });
        }

        if AMBIGUOUS_KEYS.contains(&key) {
            return Ok(LabelSuggestion {
                category_slug: None,
                proposed_path: None,
                confidence: 0.6,
                ambiguous: true,
                reasoning: Some("fake provider: multiple plausible categories".to_string()),
            });
        }

        if let Some((_, slug, confidence)) = KEYWORD_TABLE.iter().find(|(k, _, _)| *k == key) {
            return Ok(LabelSuggestion {
                category_slug: Some((*slug).to_string()),
                proposed_path: None,
                confidence: *confidence,
                ambiguous: false,
                reasoning: Some(format!("fake provider: keyword match on {key:?}")),
            });
        }

        Ok(LabelSuggestion {
            category_slug: hash_to_catalog_slug(key, req.catalog),
            proposed_path: None,
            confidence: FALLBACK_CONFIDENCE,
            ambiguous: false,
            reasoning: Some(format!("fake provider: no keyword match for {key:?}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::CatalogEntry;
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    fn catalog() -> CategoryCatalog {
        CategoryCatalog::new(vec![
            CatalogEntry {
                slug: "food.groceries".to_string(),
                name: "Groceries".to_string(),
                kind: "expense".to_string(),
            },
            CatalogEntry {
                slug: "transportation.gas".to_string(),
                name: "Gas".to_string(),
                kind: "expense".to_string(),
            },
            CatalogEntry {
                slug: "entertainment.subscriptions".to_string(),
                name: "Subscriptions".to_string(),
                kind: "expense".to_string(),
            },
        ])
    }

    fn request<'a>(counterparty: Option<&'a str>, catalog: &'a CategoryCatalog) -> LabelRequest<'a> {
        LabelRequest {
            counterparty,
            description: None,
            amount: Decimal::new(-1250, 2),
            currency: "EUR",
            booking_date: NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            transaction_type: None,
            catalog,
            prompt_version: "2",
        }
    }

    #[tokio::test]
    async fn known_keyword_resolves_deterministically() {
        let catalog = catalog();
        let provider = FakeProvider::new();
        let req = request(Some("rewe"), &catalog);

        let first = provider.suggest(&req).await.unwrap();
        let second = provider.suggest(&req).await.unwrap();

        assert_eq!(first, second);
        assert_eq!(first.category_slug.as_deref(), Some("food.groceries"));
        assert_eq!(first.confidence, 0.97);
        assert!(!first.ambiguous);
    }

    #[tokio::test]
    async fn unknown_keyword_is_deterministic_and_below_review_threshold() {
        let catalog = catalog();
        let provider = FakeProvider::new();
        let req = request(Some("some totally novel merchant xyz"), &catalog);

        let first = provider.suggest(&req).await.unwrap();
        let second = provider.suggest(&req).await.unwrap();

        assert_eq!(first, second);
        assert!(first.category_slug.is_some());
        assert_eq!(first.confidence, FALLBACK_CONFIDENCE);
        assert!(first.confidence < 0.5);
    }

    #[tokio::test]
    async fn ambiguous_fixture_key_is_flagged_ambiguous() {
        let catalog = catalog();
        let provider = FakeProvider::new();
        let req = request(Some("amazon"), &catalog);

        let suggestion = provider.suggest(&req).await.unwrap();

        assert!(suggestion.ambiguous);
        assert!(suggestion.category_slug.is_none());
    }

    #[tokio::test]
    async fn new_category_fixture_key_proposes_a_path_and_never_a_slug() {
        let catalog = catalog();
        let provider = FakeProvider::new();
        let req = request(Some("acme co-working"), &catalog);

        let suggestion = provider.suggest(&req).await.unwrap();

        assert!(suggestion.category_slug.is_none());
        assert_eq!(suggestion.proposed_path.as_deref(), Some("housing.coworking"));
    }

    #[tokio::test]
    async fn empty_counterparty_falls_back_to_description() {
        let catalog = catalog();
        let provider = FakeProvider::new();
        let mut req = request(Some(""), &catalog);
        req.description = Some("rewe");

        let suggestion = provider.suggest(&req).await.unwrap();

        assert_eq!(suggestion.category_slug.as_deref(), Some("food.groceries"));
    }

    #[test]
    fn id_and_model_are_stable() {
        let provider = FakeProvider::new();
        assert_eq!(provider.id(), "fake");
        assert_eq!(provider.model(), "fake-v1");
    }
}
