//! The §2.9 LLM provider contract, frozen by WP0 so WP1 can implement the
//! real `anthropic`/`ollama`/`openai` providers against it in parallel with
//! everyone else. Only this file and [`fake`] are WP0-owned; the rest of this
//! crate (including the prompt rewrite and the provider factory that reads
//! `APP_llm_provider`) belongs to WP1.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::fmt;

pub mod fake;

/// One node of the seeded category taxonomy, as the catalog presents it to a
/// provider: slug, display name and kind, flattened to at most three levels
/// (§3 `category.depth`). Providers never see parent/child structure, only
/// the flat list of slugs they may choose from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub slug: String,
    pub name: String,
    /// `income` | `expense` | `transfer` | `saving` (§3 `category.kind`).
    pub kind: String,
}

/// The slug catalog handed to a provider alongside a [`LabelRequest`]. A thin
/// wrapper so the fake (and later the real providers' prompt renderer) can
/// look a slug up without re-walking the category tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CategoryCatalog {
    pub entries: Vec<CatalogEntry>,
}

impl CategoryCatalog {
    pub fn new(entries: Vec<CatalogEntry>) -> Self {
        Self { entries }
    }

    pub fn contains_slug(&self, slug: &str) -> bool {
        self.entries.iter().any(|entry| entry.slug == slug)
    }
}

/// One transaction to categorize. Lifetime-generic because every field is
/// borrowed from the caller's already-normalized data (§2.5
/// `labeling::normalize`); the provider never owns a copy.
#[derive(Debug, Clone)]
pub struct LabelRequest<'a> {
    pub counterparty: Option<&'a str>,
    pub description: Option<&'a str>,
    pub amount: Decimal,
    pub currency: &'a str,
    pub booking_date: NaiveDate,
    pub transaction_type: Option<&'a str>,
    /// Slug + name + kind, at most 3 levels (§3 `category.depth`).
    pub catalog: &'a CategoryCatalog,
    /// Part of the §2.5 cache fingerprint; bumping it invalidates the cache.
    pub prompt_version: &'a str,
}

/// A provider's answer to a [`LabelRequest`]. Never constructed directly from
/// untrusted provider JSON without going through `confidence.clamp(0.0,
/// 1.0)` first (§2.9).
#[derive(Debug, Clone, PartialEq)]
pub struct LabelSuggestion {
    /// `None` means the model declined to pick any known slug.
    pub category_slug: Option<String>,
    /// A dotted path the model wants created as a new category. Never
    /// auto-created (§2.4) — always surfaces as `review_reason=new_category`.
    pub proposed_path: Option<String>,
    pub confidence: f32,
    pub ambiguous: bool,
    pub reasoning: Option<String>,
}

/// Failure modes a [`LabelProvider`] can report. A provider error or timeout
/// publishes nothing (§2.5 edge cases) — it is never mapped to a label, only
/// ever to one of these variants so the labeler can tell "no answer yet" from
/// "a broken answer".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// HTTP 429 — caller may retry once, jittered (§2.9).
    RateLimited,
    /// HTTP 5xx.
    ServerError(String),
    /// `APP_llm_timeout_ms` elapsed without a response.
    Timeout,
    /// The response did not parse as the expected structured JSON.
    Parse(String),
    /// The provider could not be constructed from the current settings (e.g.
    /// `anthropic` selected without `APP_anthropic_api_key`). Raised lazily at
    /// the factory (§4), never at `webapp`/projector startup.
    Configuration(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::RateLimited => write!(f, "provider rate-limited the request"),
            ProviderError::ServerError(msg) => write!(f, "provider server error: {msg}"),
            ProviderError::Timeout => write!(f, "provider request timed out"),
            ProviderError::Parse(msg) => write!(f, "provider response did not parse: {msg}"),
            ProviderError::Configuration(msg) => write!(f, "provider misconfigured: {msg}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// A pluggable source of category suggestions (§2.9). Implementations:
/// `fake` (WP0, this module), `anthropic`/`ollama`/`openai` (WP1).
#[async_trait::async_trait]
pub trait LabelProvider: Send + Sync {
    /// `"anthropic"` | `"ollama"` | `"openai"` | `"fake"` — also the
    /// `provider` column/field stamped on every label and cache record.
    fn id(&self) -> &'static str;
    fn model(&self) -> &str;
    async fn suggest(&self, req: &LabelRequest<'_>) -> Result<LabelSuggestion, ProviderError>;
}
