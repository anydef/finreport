//! §2.5 resolution chain: split → user override → rule → LLM cache → LLM,
//! in that precedence order, each step short-circuiting the ones below it.
//!
//! WP0 stub — owned by WP2. WP3's `processor` calls this against the
//! projection; WP4's GraphQL resolvers may call it directly for read paths
//! that need up-to-the-millisecond resolution without waiting on the labeler.
//!
//! **Split into two pure functions, not one**, because the LLM call itself
//! (§2.9) is network I/O that belongs to WP3's processor (via WP1's
//! [`categorizer::provider::LabelProvider`]), not to this "no Kafka, no
//! actix" module:
//! - [`resolve_without_llm`] covers split/override/rule/cache-hit and
//!   returns `None` only when the caller must actually ask the LLM.
//! - [`resolve_llm_answer`] turns that provider answer (or an equivalent
//!   cache hit) into the same [`Resolution`] shape, so both paths share one
//!   classification of "known slug" / "unknown slug" / "ambiguous or
//!   low-confidence".
//!
//! The WP0 stub's `Resolution` carried only the `transaction_label` columns
//! every source can fill in; it is extended here with the LLM/cache-specific
//! columns (§2.4: `provider`, `model`, `prompt_version`, `fingerprint`,
//! `reasoning`, `proposed_category_path`) a real labeler needs to build the
//! full `LabelRecord` — permitted per `labeling/mod.rs`'s "WP2/WP3 own
//! changing these signatures if the real implementation needs a different
//! shape".

use categorizer::provider::LabelSuggestion;

use crate::kafka::labeling::{CacheRecord, LabelSource, LabelStatus, ReviewReason, RuleRecord};

/// One resolved (or held-for-review) outcome of the §2.5 chain, independent
/// of how it is eventually serialized onto `finreport.transaction-label`.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    pub status: LabelStatus,
    pub source: LabelSource,
    pub category_slug: Option<String>,
    pub review_reason: Option<ReviewReason>,
    pub rule_id: Option<uuid::Uuid>,
    /// `None` unless `source` is `LlmCache`/`Llm` (§2.4).
    pub confidence: Option<f32>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    pub fingerprint: Option<String>,
    pub reasoning: Option<String>,
    /// An LLM suggestion that is **never** auto-created (§2.4).
    pub proposed_category_path: Option<String>,
}

impl Resolution {
    fn source_only(source: LabelSource, category_slug: Option<String>, rule_id: Option<uuid::Uuid>) -> Self {
        Resolution {
            status: LabelStatus::Resolved,
            source,
            category_slug,
            review_reason: None,
            rule_id,
            confidence: None,
            provider: None,
            model: None,
            prompt_version: None,
            fingerprint: None,
            reasoning: None,
            proposed_category_path: None,
        }
    }
}

/// Whatever the §2.5 chain's first four steps have already loaded from the
/// projection (split flag, override, matched rule, cache hit) for one
/// transaction.
#[derive(Debug, Clone, Default)]
pub struct ChainInputs<'a> {
    /// A split transaction has no single category; its parts carry theirs
    /// (§2.5 step 1).
    pub is_split: bool,
    /// `Some(slug)` ⇒ an active user override. `None` covers both "never
    /// overridden" and "override cleared" — both fall through to the rule
    /// step, which is the point of the clear (§2.6).
    pub user_category_slug: Option<&'a str>,
    /// The already-computed `rules::most_specific_match` winner, if any.
    pub matched_rule: Option<&'a RuleRecord>,
    /// An exact-`fingerprint` hit on `finreport.llm-cache`, if any.
    pub cache_hit: Option<&'a CacheRecord>,
}

/// Runs the first four steps of the §2.5 chain — split, user override, rule,
/// LLM cache — short-circuiting at the first that applies. Returns `None`
/// only when none of the four apply and the caller must call the LLM
/// (§2.9) and pass its answer to [`resolve_llm_answer`].
pub fn resolve_without_llm(
    inputs: &ChainInputs<'_>,
    min_confidence: f32,
    is_known_slug: &dyn Fn(&str) -> bool,
) -> Option<Resolution> {
    if inputs.is_split {
        return Some(Resolution::source_only(LabelSource::User, None, None));
    }
    if let Some(slug) = inputs.user_category_slug {
        return Some(Resolution::source_only(
            LabelSource::User,
            Some(slug.to_string()),
            None,
        ));
    }
    if let Some(rule) = inputs.matched_rule {
        return Some(Resolution::source_only(
            LabelSource::Rule,
            Some(rule.category_slug.clone()),
            Some(rule.id),
        ));
    }
    if let Some(cache) = inputs.cache_hit {
        return Some(classify(
            cache.category_slug.as_deref(),
            cache.proposed_path.as_deref(),
            cache.confidence,
            // CacheRecord does not retain the provider's `ambiguous` flag
            // (§2.4 only mirrors `llm_label_cache`'s columns); an
            // originally-ambiguous answer was cached with no category slug
            // and no proposed path, so the `None, None` branch of
            // `classify` reaches the same `Ambiguous` outcome without it.
            false,
            min_confidence,
            is_known_slug,
            LabelSource::LlmCache,
            Some(cache.provider.clone()),
            Some(cache.model.clone()),
            Some(cache.prompt_version.clone()),
            cache.reasoning.clone(),
            Some(cache.fingerprint.clone()),
        ));
    }
    None
}

/// Turns a fresh LLM answer (§2.9 `LabelSuggestion`) into a [`Resolution`],
/// stamping `label_source = llm` and the identity of the call that produced
/// it (§2.4: `provider`, `model`, `prompt_version`, `fingerprint`).
pub fn resolve_llm_answer(
    suggestion: &LabelSuggestion,
    provider: &str,
    model: &str,
    prompt_version: &str,
    fingerprint: &str,
    min_confidence: f32,
    is_known_slug: &dyn Fn(&str) -> bool,
) -> Resolution {
    classify(
        suggestion.category_slug.as_deref(),
        suggestion.proposed_path.as_deref(),
        suggestion.confidence,
        suggestion.ambiguous,
        min_confidence,
        is_known_slug,
        LabelSource::Llm,
        Some(provider.to_string()),
        Some(model.to_string()),
        Some(prompt_version.to_string()),
        suggestion.reasoning.clone(),
        Some(fingerprint.to_string()),
    )
}

/// §2.5 step 5's three-way classification, shared by a fresh LLM answer and
/// an LLM-cache hit:
/// - flagged ambiguous, or below `min_confidence` ⇒
///   `review_reason = ambiguous` (checked first: a low-confidence or
///   explicitly-ambiguous answer never resolves, no matter what slug it
///   named);
/// - otherwise a known `category_slug` ⇒ resolved;
/// - otherwise (an unknown slug, or a declined slug with a `proposed_path`)
///   ⇒ `review_reason = new_category` when there is something to show the
///   reviewer (an unknown slug or a proposed path), else `ambiguous` (the
///   model declined entirely).
#[allow(clippy::too_many_arguments)]
fn classify(
    category_slug: Option<&str>,
    proposed_path: Option<&str>,
    confidence: f32,
    ambiguous: bool,
    min_confidence: f32,
    is_known_slug: &dyn Fn(&str) -> bool,
    source: LabelSource,
    provider: Option<String>,
    model: Option<String>,
    prompt_version: Option<String>,
    reasoning: Option<String>,
    fingerprint: Option<String>,
) -> Resolution {
    let base = Resolution {
        status: LabelStatus::NeedsReview,
        source,
        category_slug: None,
        review_reason: None,
        rule_id: None,
        confidence: Some(confidence),
        provider,
        model,
        prompt_version,
        fingerprint,
        reasoning,
        proposed_category_path: proposed_path.map(str::to_string),
    };

    if ambiguous || confidence < min_confidence {
        return Resolution {
            review_reason: Some(ReviewReason::Ambiguous),
            ..base
        };
    }

    if let Some(slug) = category_slug {
        if is_known_slug(slug) {
            return Resolution {
                status: LabelStatus::Resolved,
                category_slug: Some(slug.to_string()),
                review_reason: None,
                proposed_category_path: None,
                ..base
            };
        }
        // An unknown slug is itself something to show the reviewer, same as
        // a `proposed_path` with no slug.
        return Resolution {
            review_reason: Some(ReviewReason::NewCategory),
            ..base
        };
    }

    if proposed_path.is_some() {
        return Resolution {
            review_reason: Some(ReviewReason::NewCategory),
            ..base
        };
    }

    // The model declined entirely: no slug, no proposed path.
    Resolution {
        review_reason: Some(ReviewReason::Ambiguous),
        ..base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    const MIN_CONFIDENCE: f32 = 0.5;

    fn known(slugs: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |slug: &str| slugs.contains(&slug)
    }

    fn rule(category_slug: &str) -> RuleRecord {
        RuleRecord {
            schema_version: 1,
            id: Uuid::new_v4(),
            name: "r".to_string(),
            category_slug: category_slug.to_string(),
            conditions: Default::default(),
            priority: 0,
            state: crate::kafka::labeling::RuleState::Active,
            origin: crate::kafka::labeling::RuleOrigin::User,
            auto_approved: false,
            user_touched: true,
            confidence: None,
            evidence: None,
            created_at: Utc::now(),
            revision: Utc::now(),
        }
    }

    fn cache(category_slug: Option<&str>, proposed_path: Option<&str>, confidence: f32) -> CacheRecord {
        CacheRecord {
            schema_version: 1,
            fingerprint: "fp".to_string(),
            category_slug: category_slug.map(str::to_string),
            proposed_path: proposed_path.map(str::to_string),
            confidence,
            provider: "fake".to_string(),
            model: "fake-v1".to_string(),
            prompt_version: "1".to_string(),
            reasoning: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn split_short_circuits_everything_below_it() {
        let rule_record = rule("food.groceries");
        let cache_record = cache(Some("food.restaurants"), None, 0.9);
        let inputs = ChainInputs {
            is_split: true,
            user_category_slug: Some("food.other"),
            matched_rule: Some(&rule_record),
            cache_hit: Some(&cache_record),
        };
        let resolution = resolve_without_llm(&inputs, MIN_CONFIDENCE, &known(&[])).unwrap();
        assert_eq!(resolution.source, LabelSource::User);
        assert_eq!(resolution.category_slug, None);
        assert_eq!(resolution.status, LabelStatus::Resolved);
    }

    #[test]
    fn user_override_wins_over_rule_and_cache() {
        let rule_record = rule("food.groceries");
        let cache_record = cache(Some("food.restaurants"), None, 0.9);
        let inputs = ChainInputs {
            is_split: false,
            user_category_slug: Some("food.other"),
            matched_rule: Some(&rule_record),
            cache_hit: Some(&cache_record),
        };
        let resolution = resolve_without_llm(&inputs, MIN_CONFIDENCE, &known(&[])).unwrap();
        assert_eq!(resolution.source, LabelSource::User);
        assert_eq!(resolution.category_slug.as_deref(), Some("food.other"));
        assert_eq!(resolution.confidence, None);
    }

    #[test]
    fn clearing_an_override_falls_back_to_the_rule() {
        let rule_record = rule("food.groceries");
        let inputs = ChainInputs {
            is_split: false,
            user_category_slug: None, // cleared
            matched_rule: Some(&rule_record),
            cache_hit: None,
        };
        let resolution = resolve_without_llm(&inputs, MIN_CONFIDENCE, &known(&[])).unwrap();
        assert_eq!(resolution.source, LabelSource::Rule);
        assert_eq!(resolution.category_slug.as_deref(), Some("food.groceries"));
        assert_eq!(resolution.rule_id, Some(inputs.matched_rule.unwrap().id));
    }

    #[test]
    fn rule_wins_over_cache() {
        let rule_record = rule("food.groceries");
        let cache_record = cache(Some("food.restaurants"), None, 0.9);
        let inputs = ChainInputs {
            is_split: false,
            user_category_slug: None,
            matched_rule: Some(&rule_record),
            cache_hit: Some(&cache_record),
        };
        let resolution = resolve_without_llm(&inputs, MIN_CONFIDENCE, &known(&["food.restaurants"])).unwrap();
        assert_eq!(resolution.source, LabelSource::Rule);
    }

    #[test]
    fn cache_hit_with_known_slug_resolves() {
        let cache_record = cache(Some("food.restaurants"), None, 0.9);
        let inputs = ChainInputs {
            is_split: false,
            user_category_slug: None,
            matched_rule: None,
            cache_hit: Some(&cache_record),
        };
        let resolution =
            resolve_without_llm(&inputs, MIN_CONFIDENCE, &known(&["food.restaurants"])).unwrap();
        assert_eq!(resolution.source, LabelSource::LlmCache);
        assert_eq!(resolution.status, LabelStatus::Resolved);
        assert_eq!(resolution.category_slug.as_deref(), Some("food.restaurants"));
        assert_eq!(resolution.confidence, Some(0.9));
        assert_eq!(resolution.provider.as_deref(), Some("fake"));
    }

    #[test]
    fn none_of_the_four_steps_apply_requires_an_llm_call() {
        let inputs = ChainInputs {
            is_split: false,
            user_category_slug: None,
            matched_rule: None,
            cache_hit: None,
        };
        assert!(resolve_without_llm(&inputs, MIN_CONFIDENCE, &known(&[])).is_none());
    }

    #[test]
    fn llm_answer_with_known_slug_above_threshold_resolves() {
        let suggestion = LabelSuggestion {
            category_slug: Some("food.groceries".to_string()),
            proposed_path: None,
            confidence: 0.8,
            ambiguous: false,
            reasoning: Some("keyword match".to_string()),
        };
        let resolution = resolve_llm_answer(
            &suggestion,
            "fake",
            "fake-v1",
            "1",
            "fp",
            MIN_CONFIDENCE,
            &known(&["food.groceries"]),
        );
        assert_eq!(resolution.status, LabelStatus::Resolved);
        assert_eq!(resolution.source, LabelSource::Llm);
        assert_eq!(resolution.category_slug.as_deref(), Some("food.groceries"));
        assert_eq!(resolution.review_reason, None);
    }

    #[test]
    fn llm_answer_below_min_confidence_is_ambiguous() {
        let suggestion = LabelSuggestion {
            category_slug: Some("food.groceries".to_string()),
            proposed_path: None,
            confidence: 0.42,
            ambiguous: false,
            reasoning: None,
        };
        let resolution = resolve_llm_answer(
            &suggestion,
            "fake",
            "fake-v1",
            "1",
            "fp",
            MIN_CONFIDENCE,
            &known(&["food.groceries"]),
        );
        assert_eq!(resolution.status, LabelStatus::NeedsReview);
        assert_eq!(resolution.review_reason, Some(ReviewReason::Ambiguous));
        assert_eq!(resolution.category_slug, None);
    }

    #[test]
    fn llm_answer_flagged_ambiguous_overrides_a_known_slug() {
        let suggestion = LabelSuggestion {
            category_slug: Some("food.groceries".to_string()),
            proposed_path: None,
            confidence: 0.95,
            ambiguous: true,
            reasoning: None,
        };
        let resolution = resolve_llm_answer(
            &suggestion,
            "fake",
            "fake-v1",
            "1",
            "fp",
            MIN_CONFIDENCE,
            &known(&["food.groceries"]),
        );
        assert_eq!(resolution.review_reason, Some(ReviewReason::Ambiguous));
    }

    #[test]
    fn llm_answer_naming_unknown_slug_is_new_category() {
        let suggestion = LabelSuggestion {
            category_slug: Some("made.up.slug".to_string()),
            proposed_path: None,
            confidence: 0.9,
            ambiguous: false,
            reasoning: None,
        };
        let resolution = resolve_llm_answer(
            &suggestion,
            "fake",
            "fake-v1",
            "1",
            "fp",
            MIN_CONFIDENCE,
            &known(&["food.groceries"]),
        );
        assert_eq!(resolution.review_reason, Some(ReviewReason::NewCategory));
        assert_eq!(resolution.category_slug, None);
    }

    #[test]
    fn llm_answer_with_proposed_path_and_no_slug_is_new_category() {
        let suggestion = LabelSuggestion {
            category_slug: None,
            proposed_path: Some("food.bakery".to_string()),
            confidence: 0.9,
            ambiguous: false,
            reasoning: None,
        };
        let resolution = resolve_llm_answer(
            &suggestion,
            "fake",
            "fake-v1",
            "1",
            "fp",
            MIN_CONFIDENCE,
            &known(&["food.groceries"]),
        );
        assert_eq!(resolution.review_reason, Some(ReviewReason::NewCategory));
        assert_eq!(resolution.proposed_category_path.as_deref(), Some("food.bakery"));
    }

    #[test]
    fn llm_answer_declining_entirely_is_ambiguous() {
        let suggestion = LabelSuggestion {
            category_slug: None,
            proposed_path: None,
            confidence: 0.9,
            ambiguous: false,
            reasoning: None,
        };
        let resolution = resolve_llm_answer(
            &suggestion,
            "fake",
            "fake-v1",
            "1",
            "fp",
            MIN_CONFIDENCE,
            &known(&["food.groceries"]),
        );
        assert_eq!(resolution.review_reason, Some(ReviewReason::Ambiguous));
    }

    #[test]
    fn cache_hit_reproduces_the_same_classification_as_a_fresh_llm_answer() {
        let cache_record = cache(Some("made.up.slug"), None, 0.9);
        let inputs = ChainInputs {
            is_split: false,
            user_category_slug: None,
            matched_rule: None,
            cache_hit: Some(&cache_record),
        };
        let resolution =
            resolve_without_llm(&inputs, MIN_CONFIDENCE, &known(&["food.groceries"])).unwrap();
        assert_eq!(resolution.source, LabelSource::LlmCache);
        assert_eq!(resolution.review_reason, Some(ReviewReason::NewCategory));
    }

    #[test]
    fn confidence_exactly_at_min_confidence_resolves_not_ambiguous() {
        let suggestion = LabelSuggestion {
            category_slug: Some("food.groceries".to_string()),
            proposed_path: None,
            confidence: MIN_CONFIDENCE,
            ambiguous: false,
            reasoning: None,
        };
        let resolution = resolve_llm_answer(
            &suggestion,
            "fake",
            "fake-v1",
            "1",
            "fp",
            MIN_CONFIDENCE,
            &known(&["food.groceries"]),
        );
        assert_eq!(resolution.status, LabelStatus::Resolved);
    }
}
