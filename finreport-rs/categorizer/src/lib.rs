// §2.9: `provider` (trait + `fake`) is WP0-owned crate wiring, frozen. Every
// other module here is WP1's: the real providers, the shared HTTP/retry
// plumbing they use, the prompt renderer and the `APP_llm_provider` factory.
pub mod provider;

// Compatibility shim kept for `webapp/src/db/mod.rs` (frozen, unrelated to
// this rewrite) — see `categorize::mod`'s doc comment.
pub mod categorize;

pub mod anthropic;
pub mod factory;
pub mod http;
pub mod ollama;
pub mod openai;
pub mod prompt;
