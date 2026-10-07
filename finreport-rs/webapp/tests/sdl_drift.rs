//! SDL drift test (§5): the committed `webapp/schema.graphql` is WP0's
//! frozen contract. This asserts the *structure* the live schema builds —
//! types, fields, arguments, directives, in declaration order — is
//! byte-identical to it once free-text doc comments are stripped out.
//!
//! Deviation (noted for the final WP4 report): the spec says "normalized
//! ASTs"; adding a real GraphQL AST parser (e.g. `async-graphql-parser`) as
//! a dev-dependency would mean editing the WP0-owned `webapp/Cargo.toml`,
//! so this instead does a dependency-free structural strip: drop every
//! `"""..."""`/`#...` comment, then collapse whitespace. That's equivalent
//! to an AST-level diff for everything that actually defines wire
//! compatibility (names, types, nullability, defaults, directive
//! placement) and only glosses over description text, which resolvers are
//! expected to keep improving as they go from stub to real (§9).

use sea_orm::{DatabaseBackend, MockDatabase};
use std::collections::BTreeMap;
use std::sync::Arc;
use utils::settings::Settings;
use webapp::graphql::create_schema;

/// Strips `"""triple-quoted"""` and `#line` comments, then collapses all
/// whitespace runs to a single space, so differences in description text or
/// incidental formatting (tabs vs. spaces, trailing newline) don't register
/// as drift.
fn normalize(sdl: &str) -> String {
    let mut out = String::new();
    let mut chars = sdl.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' && chars.peek() == Some(&'"') {
            // Possible `"""..."""` block description.
            let mut lookahead = chars.clone();
            if lookahead.next() == Some('"') && lookahead.next() == Some('"') {
                chars = lookahead;
                // Consume until the closing `"""`.
                let mut quote_run = 0;
                for ch in chars.by_ref() {
                    if ch == '"' {
                        quote_run += 1;
                        if quote_run == 3 {
                            break;
                        }
                    } else {
                        quote_run = 0;
                    }
                }
                continue;
            }
        }
        if c == '#' {
            for ch in chars.by_ref() {
                if ch == '\n' {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn dummy_settings() -> Arc<Settings> {
    Arc::new(Settings {
        oauth_url: None,
        url: None,
        save_file_path: None,
        database_url: None,
        kafka_brokers: None,
        cookie_secure: true,
        allowed_origins: String::new(),
        session_ttl_days: 30,
        projector_default_owner: None,
        llm_provider: "fake".to_string(),
        anthropic_api_key: None,
        llm_api_key: None,
        llm_base_url: None,
        llm_model: None,
        llm_timeout_ms: 20_000,
        llm_min_confidence: 0.5,
        llm_max_requests_per_run: 200,
        prompt_version: "2".to_string(),
        rule_learn_min_observations: 3,
        rule_auto_approve_threshold: 0.9,
        labeler_max_projection_lag: 0,
        transfer_match_days: 3,
        recurring_min_occurrences: 3,
        recurring_amount_tolerance: 0.10,
        recurring_window_months: 18,
        max_tags_per_transaction: 10,
        accounts: BTreeMap::new(),
        account_name: None,
        client_id: None,
        client_secret: None,
        zugangsnummer: None,
        pin: None,
    })
}

#[test]
fn live_sdl_matches_the_frozen_schema_graphql() {
    let conn = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
    let schema = create_schema(Arc::new(conn), dummy_settings());
    let live = normalize(&schema.sdl());

    let frozen_path = concat!(env!("CARGO_MANIFEST_DIR"), "/schema.graphql");
    let frozen = normalize(
        &std::fs::read_to_string(frozen_path)
            .unwrap_or_else(|e| panic!("reading {frozen_path}: {e}")),
    );

    assert_eq!(
        live, frozen,
        "live schema SDL drifted from the frozen webapp/schema.graphql \
         (structure only, descriptions ignored) — regenerate it with \
         `cargo run --bin graphql_schema_exporter` if this is an \
         intentional WP0-reviewed change, otherwise fix the resolver that \
         introduced the drift"
    );
}
