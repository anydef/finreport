//! One-off repair: categories created with a bare leaf slug *and* a parent.
//!
//! A category's place in the tree is encoded in its slug (`leisure.hobbies.games`)
//! and every descendant match is a dotted whole-segment prefix match on it.
//! `create_category` once accepted a child whose slug was not prefixed by its
//! parent's, leaving it invisible to rollups, the breakdown, filters and goals.
//!
//! Correcting a slug changes the id (`category_uuid(slug)`), and everything that
//! references a category does so by **slug** on compacted topics
//! (`finreport.user-label`, `finreport.rule`, `finreport.category`). Repointing
//! Postgres alone would be undone by the next replay, so [`run`] republishes the
//! affected events with the corrected slug and applies the same change to the
//! projection (publish-then-upsert, like the GraphQL mutations).
//!
//! Order per broken category (every step is idempotent, so a partial run is
//! safe to re-run):
//!
//! 1. publish + project the corrected category (skipped when it already exists);
//! 2. republish + project every affected `user-label` record (whole-state, so
//!    tags, `recurring`, `note` and split parts are preserved);
//! 3. republish + project every affected `rule` record;
//! 4. delete the stale `transaction_label` rows (the labeler's sweep
//!    re-resolves them through the now-correct overrides/rules);
//! 5. tombstone + delete the broken category — last, once nothing points at it.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;

use chrono::Utc;
use entity::entities::{category, transaction_label};
use rdkafka::message::{Header, OwnedHeaders};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use serde::de::DeserializeOwned;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::kafka::envelope::{transaction_uuid, HEADER_ORIGIN, HEADER_SCHEMA_VERSION};
use crate::kafka::labeling::{
    category_uuid, CategoryKind, CategoryOrigin, CategoryRecord, RuleRecord, UserLabelRecord,
    CURRENT_SCHEMA_VERSION, ORIGIN_USER, TOPIC_CATEGORY, TOPIC_RULE, TOPIC_USER_LABEL,
};
use crate::kafka::producer::EventPublisher;
use crate::kafka::scan::{scan_topic, ScannedRecord};
use crate::projection::labeling::{project_category, project_rule, project_user_label};

type BoxError = Box<dyn Error + Send + Sync>;

/// Deepest slug the tree allows (`a.b.c`), matching `create_category`.
const MAX_DEPTH: usize = 3;

// ---------------------------------------------------------------------------
// Pure: detection and slug arithmetic
// ---------------------------------------------------------------------------

/// A category with a parent whose slug is not `parent.slug + "."`-prefixed.
/// Top-level categories (no parent) are never broken.
pub fn is_broken(slug: &str, parent_slug: Option<&str>) -> bool {
    parent_slug.is_some_and(|parent| !slug.starts_with(&format!("{parent}.")))
}

/// `parent.slug + "." + <last segment of the current slug>`.
pub fn corrected_slug(slug: &str, parent_slug: &str) -> String {
    let leaf = slug.rsplit('.').next().unwrap_or(slug);
    format!("{parent_slug}.{leaf}")
}

/// `^[a-z0-9_]+(\.[a-z0-9_]+){0,2}$`, hand-checked (see `graphql::categories`).
fn is_valid_slug(slug: &str) -> bool {
    let segments: Vec<&str> = slug.split('.').collect();
    segments.len() <= MAX_DEPTH
        && segments.iter().all(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
}

fn depth_of(slug: &str) -> i16 {
    (slug.matches('.').count() + 1) as i16
}

/// What to do about one broken category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The corrected slug is free: publish a new category.
    Create,
    /// The corrected slug already exists (same kind and parent): reuse it.
    Merge { existing_id: Uuid },
    /// Left untouched, with the reason (surfaced in the report).
    Skip(String),
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub broken: category::Model,
    pub parent_slug: String,
    pub new_slug: String,
    pub action: Action,
}

/// Finds every broken category in `rows` and decides what to do with each.
/// `only` (when non-empty) restricts the result to those *current* slugs.
pub fn plan_repairs(rows: &[category::Model], only: &[String]) -> Vec<Plan> {
    let by_id: HashMap<Uuid, &category::Model> = rows.iter().map(|r| (r.id, r)).collect();
    let by_slug: HashMap<&str, &category::Model> =
        rows.iter().map(|r| (r.slug.as_str(), r)).collect();
    let row_is_broken = |row: &category::Model| {
        row.parent_id
            .and_then(|pid| by_id.get(&pid))
            .is_some_and(|p| is_broken(&row.slug, Some(&p.slug)))
    };

    let mut claimed: HashMap<String, Uuid> = HashMap::new();
    let mut plans = Vec::new();
    let mut sorted: Vec<&category::Model> = rows.iter().collect();
    sorted.sort_by(|a, b| a.slug.cmp(&b.slug));

    for row in sorted {
        let Some(parent) = row.parent_id.and_then(|pid| by_id.get(&pid)) else {
            continue;
        };
        if !is_broken(&row.slug, Some(&parent.slug)) {
            continue;
        }
        if !only.is_empty() && !only.contains(&row.slug) {
            continue;
        }
        let new_slug = corrected_slug(&row.slug, &parent.slug);
        let make = |action: Action| Plan {
            broken: row.clone(),
            parent_slug: parent.slug.clone(),
            new_slug: new_slug.clone(),
            action,
        };

        if !is_valid_slug(&new_slug) {
            plans.push(make(Action::Skip(format!(
                "corrected slug '{new_slug}' is not a valid slug (too deep?)"
            ))));
            continue;
        }
        if row_is_broken(parent) {
            plans.push(make(Action::Skip(format!(
                "parent '{}' is itself broken; repair it first",
                parent.slug
            ))));
            continue;
        }
        if rows.iter().any(|r| r.parent_id == Some(row.id)) {
            plans.push(make(Action::Skip(
                "has child categories whose parent link would dangle; fix by hand".into(),
            )));
            continue;
        }
        if let Some(other) = claimed.get(&new_slug) {
            plans.push(make(Action::Skip(format!(
                "another broken category ({other}) maps to '{new_slug}' in this run; re-run afterwards"
            ))));
            continue;
        }
        let action = match by_slug.get(new_slug.as_str()) {
            None => Action::Create,
            Some(existing) if existing.kind == row.kind && existing.parent_id == row.parent_id => {
                Action::Merge {
                    existing_id: existing.id,
                }
            }
            Some(existing) => {
                plans.push(make(Action::Skip(format!(
                    "'{new_slug}' already exists with a different kind ({}) or parent, not merging",
                    existing.kind
                ))));
                continue;
            }
        };
        claimed.insert(new_slug.clone(), row.id);
        plans.push(make(action));
    }
    plans
}

// ---------------------------------------------------------------------------
// Pure: rewriting the slug-carrying records
// ---------------------------------------------------------------------------

/// How many slots of one user-label record referenced the broken slug.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LabelHits {
    pub override_hit: bool,
    pub parts: usize,
}

impl LabelHits {
    pub fn any(&self) -> bool {
        self.override_hit || self.parts > 0
    }
}

pub fn label_hits(record: &UserLabelRecord, old: &str) -> LabelHits {
    LabelHits {
        override_hit: record.category_slug.as_deref() == Some(old),
        parts: record
            .parts
            .iter()
            .filter(|p| p.category_slug == old)
            .count(),
    }
}

/// Rewrites only the slug fields; tags, recurring, note, amounts and indexes
/// are left exactly as they were.
pub fn reslug_user_label(record: &mut UserLabelRecord, old: &str, new: &str) {
    if record.category_slug.as_deref() == Some(old) {
        record.category_slug = Some(new.to_string());
    }
    for part in record.parts.iter_mut().filter(|p| p.category_slug == old) {
        part.category_slug = new.to_string();
    }
}

// ---------------------------------------------------------------------------
// Kafka state
// ---------------------------------------------------------------------------

struct Held<T> {
    record: T,
    headers: Vec<(String, String)>,
}

/// The latest live record per key of a compacted topic, plus the text of any
/// payload that did not parse (so a record we cannot read but that mentions a
/// broken slug stops the run instead of being silently left behind).
struct Latest<T> {
    by_key: BTreeMap<String, Held<T>>,
    unparseable: Vec<String>,
}

fn load_latest<T: DeserializeOwned>(brokers: &str, topic: &str) -> Result<Latest<T>, BoxError> {
    Ok(latest_of(scan_topic(brokers, topic)?))
}

fn latest_of<T: DeserializeOwned>(scanned: Vec<ScannedRecord>) -> Latest<T> {
    let mut by_key = BTreeMap::new();
    let mut unparseable = Vec::new();
    for rec in scanned {
        let Some(key) = rec.key else { continue };
        match rec.payload {
            None => {
                by_key.remove(&key);
            }
            Some(bytes) => match serde_json::from_slice::<T>(&bytes) {
                Ok(record) => {
                    by_key.insert(
                        key,
                        Held {
                            record,
                            headers: rec.headers,
                        },
                    );
                }
                Err(_) => {
                    by_key.remove(&key);
                    unparseable.push(String::from_utf8_lossy(&bytes).into_owned());
                }
            },
        }
    }
    Latest {
        by_key,
        unparseable,
    }
}

fn headers_of(pairs: &[(String, String)]) -> OwnedHeaders {
    pairs.iter().fold(OwnedHeaders::new(), |h, (k, v)| {
        h.insert(Header {
            key: k,
            value: Some(v.as_str()),
        })
    })
}

fn user_headers() -> OwnedHeaders {
    OwnedHeaders::new()
        .insert(Header {
            key: HEADER_ORIGIN,
            value: Some(ORIGIN_USER),
        })
        .insert(Header {
            key: HEADER_SCHEMA_VERSION,
            value: Some(CURRENT_SCHEMA_VERSION.to_string().as_str()),
        })
}

// ---------------------------------------------------------------------------
// The repair
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub dry_run: bool,
    /// Restrict to these *current* (broken) slugs; empty = every broken one.
    pub only: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryReport {
    pub old_slug: String,
    pub new_slug: String,
    pub action: Action,
    /// `transaction_label` rows pointing at the broken category.
    pub labels: u64,
    /// `user-label` records whose own category is the broken one.
    pub overrides: usize,
    /// Split parts (across all `user-label` records) on the broken category.
    pub splits: usize,
    pub rules: usize,
}

fn kafka_kind(kind: &str) -> Result<CategoryKind, BoxError> {
    Ok(serde_json::from_value(serde_json::Value::String(
        kind.to_string(),
    ))?)
}

fn kafka_origin(origin: &str) -> CategoryOrigin {
    match origin {
        "seed" => CategoryOrigin::Seed,
        _ => CategoryOrigin::User,
    }
}

/// Discovers broken categories, reports them and (unless `opts.dry_run`)
/// repairs them. `publisher` must be `Some` for a real run.
pub async fn run(
    db: &DatabaseConnection,
    brokers: &str,
    publisher: Option<&EventPublisher>,
    opts: &Options,
) -> Result<Vec<CategoryReport>, BoxError> {
    if !opts.dry_run && publisher.is_none() {
        return Err("a real run needs a publisher".into());
    }
    let rows = category::Entity::find().all(db).await?;
    let plans = plan_repairs(&rows, &opts.only);
    if plans.is_empty() {
        info!("[category-reslug] no broken categories found (idempotent re-run)");
        return Ok(Vec::new());
    }

    let mut user_labels = load_latest::<UserLabelRecord>(brokers, TOPIC_USER_LABEL)?;
    let mut rules = load_latest::<RuleRecord>(brokers, TOPIC_RULE)?;

    for plan in plans
        .iter()
        .filter(|p| !matches!(p.action, Action::Skip(_)))
    {
        let quoted = format!("\"{}\"", plan.broken.slug);
        if user_labels
            .unparseable
            .iter()
            .chain(&rules.unparseable)
            .any(|t| t.contains(&quoted))
        {
            return Err(format!(
                "an unreadable user-label/rule record mentions '{}'; refusing to run so it is not orphaned",
                plan.broken.slug
            )
            .into());
        }
    }

    let mut reports = Vec::new();
    for plan in &plans {
        let old = plan.broken.slug.as_str();
        let new = plan.new_slug.as_str();

        let labels = transaction_label::Entity::find()
            .filter(transaction_label::Column::CategoryId.eq(plan.broken.id))
            .count(db)
            .await?;
        let label_keys: Vec<String> = user_labels
            .by_key
            .iter()
            .filter(|(_, h)| label_hits(&h.record, old).any())
            .map(|(k, _)| k.clone())
            .collect();
        let hits: Vec<LabelHits> = label_keys
            .iter()
            .map(|k| label_hits(&user_labels.by_key[k].record, old))
            .collect();
        let rule_keys: Vec<String> = rules
            .by_key
            .iter()
            .filter(|(_, h)| h.record.category_slug == old)
            .map(|(k, _)| k.clone())
            .collect();

        let report = CategoryReport {
            old_slug: old.to_string(),
            new_slug: new.to_string(),
            action: plan.action.clone(),
            labels,
            overrides: hits.iter().filter(|h| h.override_hit).count(),
            splits: hits.iter().map(|h| h.parts).sum(),
            rules: rule_keys.len(),
        };
        log_report(&report, opts.dry_run);

        if let (false, false, Some(publisher)) = (
            opts.dry_run,
            matches!(plan.action, Action::Skip(_)),
            publisher,
        ) {
            apply(
                db,
                publisher,
                plan,
                &label_keys,
                &rule_keys,
                &mut user_labels,
                &mut rules,
            )
            .await
            .inspect_err(|e| {
                error!(%old, %new, %e, "[category-reslug] failed part-way; re-run to finish")
            })?;
        }
        reports.push(report);
    }
    log_summary(&reports, opts.dry_run);
    Ok(reports)
}

async fn apply(
    db: &DatabaseConnection,
    publisher: &EventPublisher,
    plan: &Plan,
    label_keys: &[String],
    rule_keys: &[String],
    user_labels: &mut Latest<UserLabelRecord>,
    rules: &mut Latest<RuleRecord>,
) -> Result<(), BoxError> {
    let old = plan.broken.slug.as_str();
    let new = plan.new_slug.as_str();
    let new_id = category_uuid(new);

    // 1. the corrected category, before anything points at it.
    if plan.action == Action::Create {
        let record = CategoryRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: new_id,
            slug: new.to_string(),
            parent_slug: Some(plan.parent_slug.clone()),
            name: plan.broken.name.clone(),
            kind: kafka_kind(&plan.broken.kind)?,
            depth: depth_of(new),
            sort_order: plan.broken.sort_order,
            archived: plan.broken.archived,
            origin: kafka_origin(&plan.broken.origin),
            owner_user_id: plan.broken.owner_user_id,
            revision: Utc::now(),
        };
        publisher
            .publish_with_headers(
                TOPIC_CATEGORY,
                &new_id.to_string(),
                &serde_json::to_vec(&record)?,
                user_headers(),
            )
            .await?;
        project_category(db, new_id, Some(record)).await?;
    }

    // 2. the user's own decisions, whole-state.
    for key in label_keys {
        let held = user_labels
            .by_key
            .get_mut(key)
            .expect("key came from this map");
        reslug_user_label(&mut held.record, old, new);
        held.record.revision = Utc::now();
        publisher
            .publish_with_headers(
                TOPIC_USER_LABEL,
                key,
                &serde_json::to_vec(&held.record)?,
                headers_of(&held.headers),
            )
            .await?;
        let tx = transaction_uuid(&held.record.source, &held.record.external_id);
        project_user_label(db, tx, Some(held.record.clone())).await?;
    }

    // 3. rules.
    for key in rule_keys {
        let held = rules.by_key.get_mut(key).expect("key came from this map");
        held.record.category_slug = new.to_string();
        held.record.revision = Utc::now();
        publisher
            .publish_with_headers(
                TOPIC_RULE,
                key,
                &serde_json::to_vec(&held.record)?,
                headers_of(&held.headers),
            )
            .await?;
        project_rule(db, held.record.id, Some(held.record.clone())).await?;
    }

    // 4. stale labels, so the labeler's sweep re-resolves them.
    transaction_label::Entity::delete_many()
        .filter(transaction_label::Column::CategoryId.eq(plan.broken.id))
        .exec(db)
        .await?;

    // 5. the broken category, last.
    publisher
        .publish_tombstone_with_headers(
            TOPIC_CATEGORY,
            &plan.broken.id.to_string(),
            user_headers(),
        )
        .await?;
    project_category(db, plan.broken.id, None).await?;
    Ok(())
}

fn log_report(r: &CategoryReport, dry_run: bool) {
    match &r.action {
        Action::Skip(reason) => warn!(
            old = %r.old_slug, new = %r.new_slug, %reason,
            "[category-reslug] SKIPPED"
        ),
        action => info!(
            old = %r.old_slug, new = %r.new_slug,
            merged_into_existing = matches!(action, Action::Merge { .. }),
            labels = r.labels, overrides = r.overrides, splits = r.splits, rules = r.rules,
            dry_run,
            "[category-reslug] {}",
            if dry_run { "would repair" } else { "repaired" }
        ),
    }
}

fn log_summary(reports: &[CategoryReport], dry_run: bool) {
    let done: Vec<_> = reports
        .iter()
        .filter(|r| !matches!(r.action, Action::Skip(_)))
        .collect();
    info!(
        categories = done.len(),
        skipped = reports.len() - done.len(),
        overrides = done.iter().map(|r| r.overrides).sum::<usize>(),
        rules = done.iter().map(|r| r.rules).sum::<usize>(),
        dry_run,
        "[category-reslug] done"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kafka::labeling::SplitPart;
    use rust_decimal::Decimal;

    fn cat(slug: &str, parent: Option<&str>, kind: &str) -> category::Model {
        category::Model {
            id: category_uuid(slug),
            slug: slug.to_string(),
            parent_id: parent.map(category_uuid),
            name: slug.to_string(),
            kind: kind.to_string(),
            depth: depth_of(slug),
            sort_order: 0,
            archived: false,
            origin: "user".to_string(),
            owner_user_id: None,
            revision: Utc::now().into(),
        }
    }

    #[test]
    fn detects_bare_leaf_with_parent() {
        assert!(is_broken("hobby", Some("personal")));
        assert!(is_broken("gas", Some("utilities")));
    }

    #[test]
    fn correctly_prefixed_child_is_not_flagged() {
        assert!(!is_broken("personal.hobby", Some("personal")));
        assert!(!is_broken("a.b.c", Some("a.b")));
    }

    #[test]
    fn prefix_must_be_a_whole_segment() {
        assert!(is_broken("personalities.x", Some("personal")));
    }

    #[test]
    fn top_level_category_is_not_flagged() {
        assert!(!is_broken("income", None));
    }

    #[test]
    fn corrected_slug_prefixes_the_parent() {
        assert_eq!(corrected_slug("hobby", "personal"), "personal.hobby");
        assert_eq!(corrected_slug("x.y", "personal"), "personal.y");
        assert_eq!(corrected_slug("sub", "a.b"), "a.b.sub");
    }

    #[test]
    fn plan_finds_only_broken_rows() {
        let rows = vec![
            cat("personal", None, "expense"),
            cat("hobby", Some("personal"), "expense"),
            cat("personal.ok", Some("personal"), "expense"),
        ];
        let plans = plan_repairs(&rows, &[]);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].new_slug, "personal.hobby");
        assert_eq!(plans[0].action, Action::Create);
    }

    #[test]
    fn plan_merges_onto_an_existing_correct_category() {
        let rows = vec![
            cat("children", None, "expense"),
            cat("activities", Some("children"), "expense"),
            cat("children.activities", Some("children"), "expense"),
        ];
        let plans = plan_repairs(&rows, &[]);
        assert_eq!(plans.len(), 1);
        assert_eq!(
            plans[0].action,
            Action::Merge {
                existing_id: category_uuid("children.activities")
            }
        );
    }

    #[test]
    fn plan_refuses_to_merge_onto_a_different_kind() {
        let rows = vec![
            cat("children", None, "expense"),
            cat("activities", Some("children"), "expense"),
            cat("children.activities", Some("children"), "income"),
        ];
        assert!(matches!(plan_repairs(&rows, &[])[0].action, Action::Skip(_)));
    }

    #[test]
    fn plan_skips_when_corrected_slug_is_too_deep() {
        let ok = vec![cat("a.b", None, "expense"), cat("c", Some("a.b"), "expense")];
        assert_eq!(plan_repairs(&ok, &[])[0].action, Action::Create);
        let deep = vec![
            cat("a.b.c", None, "expense"),
            cat("d", Some("a.b.c"), "expense"),
        ];
        assert!(matches!(plan_repairs(&deep, &[])[0].action, Action::Skip(_)));
    }

    #[test]
    fn plan_skips_a_broken_category_that_has_children() {
        let rows = vec![
            cat("p", None, "expense"),
            cat("x", Some("p"), "expense"),
            cat("x.y", Some("x"), "expense"),
        ];
        let plans = plan_repairs(&rows, &[]);
        assert!(!plans.is_empty());
        assert!(plans.iter().all(|p| matches!(p.action, Action::Skip(_))));
    }

    #[test]
    fn only_filter_restricts_to_named_slugs() {
        let rows = vec![
            cat("p", None, "expense"),
            cat("x", Some("p"), "expense"),
            cat("y", Some("p"), "expense"),
        ];
        let plans = plan_repairs(&rows, &["y".to_string()]);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].broken.slug, "y");
    }

    fn label(category: Option<&str>, parts: &[&str]) -> UserLabelRecord {
        UserLabelRecord {
            schema_version: 2,
            source: "comdirect".into(),
            external_id: "T1".into(),
            category_slug: category.map(String::from),
            parts: parts
                .iter()
                .enumerate()
                .map(|(i, s)| SplitPart {
                    index: i as i32,
                    amount: Decimal::new(100, 2),
                    category_slug: s.to_string(),
                })
                .collect(),
            tags: vec!["a".into(), "b".into()],
            recurring: Some(true),
            revision: Utc::now(),
            note: Some("n".into()),
        }
    }

    #[test]
    fn reslug_rewrites_override_and_parts_and_nothing_else() {
        let mut rec = label(Some("gas"), &["gas", "food"]);
        let before = rec.clone();
        assert_eq!(
            label_hits(&rec, "gas"),
            LabelHits {
                override_hit: true,
                parts: 1
            }
        );
        reslug_user_label(&mut rec, "gas", "utilities.gas");
        assert_eq!(rec.category_slug.as_deref(), Some("utilities.gas"));
        assert_eq!(rec.parts[0].category_slug, "utilities.gas");
        assert_eq!(rec.parts[1].category_slug, "food");
        assert_eq!(rec.tags, before.tags);
        assert_eq!(rec.recurring, Some(true));
        assert_eq!(rec.note, before.note);
        assert_eq!(rec.parts[0].amount, before.parts[0].amount);
    }

    #[test]
    fn unrelated_label_is_not_a_hit() {
        assert!(!label_hits(&label(Some("food"), &["food"]), "gas").any());
    }

    #[test]
    fn latest_of_keeps_last_record_and_honours_tombstones() {
        let rec = |c: &str| ScannedRecord {
            key: Some("k".into()),
            payload: Some(serde_json::to_vec(&label(Some(c), &[])).unwrap()),
            headers: vec![],
        };
        let tomb = ScannedRecord {
            key: Some("k".into()),
            payload: None,
            headers: vec![],
        };
        let l = latest_of::<UserLabelRecord>(vec![rec("a"), rec("b")]);
        assert_eq!(l.by_key["k"].record.category_slug.as_deref(), Some("b"));
        let l = latest_of::<UserLabelRecord>(vec![rec("a"), tomb]);
        assert!(l.by_key.is_empty());
    }
}
