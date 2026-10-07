//! §2.5 resolution chain: split → user override → rule → LLM cache → LLM,
//! in that precedence order, each step short-circuiting the ones below it.
//!
//! WP0 stub — owned by WP2. WP3's `processor` calls this against the
//! projection; WP4's GraphQL resolvers may call it directly for read paths
//! that need up-to-the-millisecond resolution without waiting on the labeler.

use crate::kafka::labeling::{LabelSource, LabelStatus, ReviewReason};

/// One resolved (or held-for-review) outcome of the §2.5 chain, independent
/// of how it is eventually serialized onto `finreport.transaction-label`.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    pub status: LabelStatus,
    pub source: LabelSource,
    pub category_slug: Option<String>,
    pub review_reason: Option<ReviewReason>,
    pub rule_id: Option<uuid::Uuid>,
    pub confidence: Option<f32>,
}

/// Runs the §2.5 chain for one transaction against whatever rules/overrides/
/// cache the caller has already loaded from the projection.
///
/// TODO(WP2): implement per §2.5; unit-test each precedence step winning over
/// the ones below it, the split short-circuit, and an override clear falling
/// back correctly (§8 "Resolution chain" row).
pub fn resolve() -> Resolution {
    todo!("WP2: §2.5 resolution chain — split / user override / rule / llm-cache / llm precedence")
}
