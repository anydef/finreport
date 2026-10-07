//! `category-seed` (§3, §7) — reads `prompts/taxonomy.json` and idempotently
//! publishes one `finreport.category` record per node (deterministic
//! `category_uuid(slug)` ids, so re-running this with an unchanged tree is a
//! no-op beyond a `revision` bump), dual-writing into the `category`
//! projection the same way the labeler's own consume loop would. Never
//! deletes a category — including ones this seed no longer lists, which may
//! be user-created (§3 "never deletes a user-created category") or simply
//! a node dropped from a future edit of the taxonomy; removing categories is
//! an explicit, separate operation this binary does not perform.
//!
//! Root nodes are depth 1, their children depth 2 (§3 `category.depth` CHECK
//! `1..=3`; `prompts/taxonomy.json` only nests two levels deep).

use std::error::Error;
use std::fs;

use chrono::Utc;
use secrecy::ExposeSecret;
use serde::Deserialize;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use utils::settings::Settings;
use webapp::db::seaql;
use webapp::kafka::labeling::{category_uuid, CategoryKind, CategoryOrigin, CategoryRecord, TOPIC_CATEGORY};
use webapp::kafka::producer::EventPublisher;
use webapp::projection::labeling::project_category;

/// Mirrors `prompts/taxonomy.json`'s shape: a list of root nodes, each with
/// up to one level of `children`.
#[derive(Debug, Deserialize)]
struct TaxonomyNode {
    slug: String,
    name: String,
    kind: CategoryKind,
    #[serde(default)]
    children: Vec<TaxonomyNode>,
}

/// Flattens one root node (and its children) into `(depth, parent_slug,
/// node)` tuples, root-first so a child's parent is always seeded before it.
fn flatten<'a>(
    node: &'a TaxonomyNode,
    parent_slug: Option<&str>,
    depth: i16,
    out: &mut Vec<(i16, Option<String>, &'a TaxonomyNode)>,
) {
    out.push((depth, parent_slug.map(str::to_string), node));
    for child in &node.children {
        flatten(child, Some(node.slug.as_str()), depth + 1, out);
    }
}

fn taxonomy_path() -> String {
    std::env::args()
        .nth(1)
        .unwrap_or_else(|| "prompts/taxonomy.json".to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let path = taxonomy_path();
    info!(%path, "[category-seed] reading taxonomy");
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("could not read taxonomy file {path:?}: {e}"))?;
    let roots: Vec<TaxonomyNode> = serde_json::from_str(&raw)
        .map_err(|e| format!("could not parse taxonomy file {path:?}: {e}"))?;

    let mut flattened = Vec::new();
    for root in &roots {
        flatten(root, None, 1, &mut flattened);
    }
    info!(count = flattened.len(), "[category-seed] flattened taxonomy nodes");

    let settings = Settings::from_env()?;
    let brokers = settings.require_kafka_brokers()?.to_string();

    info!("[category-seed] connecting to database");
    let db = seaql::init_db_with_migrations(
        settings.require_database_url()?.expose_secret(),
        settings.run_migrations(),
    )
    .await?;

    let publisher = EventPublisher::connect(&brokers)?;
    let revision = Utc::now();
    let mut seeded = 0usize;
    let mut failed = 0usize;

    for (depth, parent_slug, node) in &flattened {
        let record = CategoryRecord {
            schema_version: webapp::kafka::labeling::CURRENT_SCHEMA_VERSION,
            id: category_uuid(&node.slug),
            slug: node.slug.clone(),
            parent_slug: parent_slug.clone(),
            name: node.name.clone(),
            kind: node.kind,
            depth: *depth,
            sort_order: 0,
            archived: false,
            origin: CategoryOrigin::Seed,
            owner_user_id: None,
            revision,
        };

        let value = match serde_json::to_vec(&record) {
            Ok(v) => v,
            Err(e) => {
                warn!(slug = %node.slug, error = %e, "[category-seed] could not serialize record, skipping");
                failed += 1;
                continue;
            }
        };
        let headers = rdkafka::message::OwnedHeaders::new()
            .insert(rdkafka::message::Header { key: "origin", value: Some("labeler") });

        if let Err(e) = publisher
            .publish_with_headers(TOPIC_CATEGORY, &record.id.to_string(), &value, headers)
            .await
        {
            warn!(slug = %node.slug, error = %e, "[category-seed] publish failed, skipping projection for this node");
            failed += 1;
            continue;
        }

        if let Err(e) = project_category(&db, record.id, Some(record.clone())).await {
            warn!(slug = %node.slug, error = %e, "[category-seed] projection failed after publish");
            failed += 1;
            continue;
        }
        seeded += 1;
    }

    info!(seeded, failed, "[category-seed] done");
    if failed > 0 {
        return Err(format!("{failed} categor{} failed to seed", if failed == 1 { "y" } else { "ies" }).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses the real `prompts/taxonomy.json` (not a fixture copy, so this
    /// test breaks the moment the taxonomy and this flattener disagree) and
    /// checks the depth/parent invariants §3's `category` CHECK constraints
    /// rely on.
    #[test]
    fn flattens_the_real_taxonomy_with_valid_depths_and_parents() {
        let raw = fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../prompts/taxonomy.json"
        ))
        .expect("prompts/taxonomy.json must exist");
        let roots: Vec<TaxonomyNode> = serde_json::from_str(&raw).unwrap();
        assert!(!roots.is_empty());

        let mut flattened = Vec::new();
        for root in &roots {
            flatten(root, None, 1, &mut flattened);
        }

        // 16 roots + however many children the fixture carries today; this
        // is really just "every node reachable from a root was visited".
        let expected_total: usize = roots.iter().map(|r| 1 + r.children.len()).sum();
        assert_eq!(flattened.len(), expected_total);

        for (depth, parent_slug, node) in &flattened {
            assert!((1..=3).contains(depth), "slug {} has depth {depth}", node.slug);
            if *depth == 1 {
                assert!(parent_slug.is_none(), "root {} must have no parent", node.slug);
            } else {
                assert!(parent_slug.is_some(), "child {} must have a parent", node.slug);
            }
            // `category_uuid` is deterministic by slug alone: re-running the
            // seed twice must compute the same id for the same slug.
            assert_eq!(category_uuid(&node.slug), category_uuid(&node.slug));
        }
    }
}
