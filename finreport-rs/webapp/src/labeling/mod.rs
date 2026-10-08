//! Iteration 2's resolution chain, rules engine and learner (§2.5, §2.7,
//! §2.8), plus the labeler's own processing loop (§2.3). No Kafka, no actix
//! below the `processor` module — pure, unit-testable logic so WP4 (GraphQL)
//! can call it directly instead of only reading the projection.
//!
//! **Ownership (frozen by WP0, see `docs/specs/iteration-2.md` §10):**
//! - `normalize`, `fingerprint`, `rules`, `learn` — WP2.
//! - The §2.5 precedence chain lives only in `processor::resolve_transaction`;
//!   a once-separate slug-based `resolve` module duplicated it, was called
//!   from nowhere, and let the live path drift into trusting unvalidated
//!   slugs, so it was deleted.
//! - `processor` — WP3, depends on WP2's signatures.
//!
//! Every function below is a WP0 stub: a signature inferred from the prose in
//! §2.5/§2.7/§2.8 (the spec gives no code block for this module, unlike
//! §2.9's provider trait), `todo!()`-bodied so the workspace compiles while
//! WP2/WP3 build the real implementations in parallel. WP2/WP3 own changing
//! these signatures if the real implementation needs a different shape.

pub mod fingerprint;
pub mod learn;
pub mod normalize;
pub mod processor;
pub mod rules;
