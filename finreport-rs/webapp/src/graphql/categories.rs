//! Category tree (§5): listing, creation, rename, archive. Every write
//! publishes a `finreport.category` record and upserts the `category`
//! projection in the same request (§2.1 publish-then-upsert).

use async_graphql::ErrorExtensions;
use chrono::Utc;
use entity::entities::category;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    Statement,
};
use std::sync::Arc;

use crate::graphql::events::{kafka_unavailable_error, publish_event};
use crate::graphql::scalars::Uuid as GqlUuid;
use crate::graphql::types::{Category, CategoryInput, CategoryKind as GqlCategoryKind};
use crate::kafka::labeling::{
    category_uuid, CategoryKind as KafkaCategoryKind, CategoryOrigin, CategoryRecord,
    CURRENT_SCHEMA_VERSION, TOPIC_CATEGORY,
};
use crate::kafka::producer::EventPublisher;

fn validation_error(message: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(message.into()).extend_with(|_, e| e.set("code", "VALIDATION"))
}

/// `^[a-z0-9_]+(\.[a-z0-9_]+){0,2}$` (§5), hand-checked: `webapp` has no
/// direct `regex` dependency (`Cargo.toml` is frozen by WP0), so this is a
/// manual character-class walk over dot-separated segments rather than a
/// compiled pattern.
pub fn is_valid_slug(slug: &str) -> bool {
    let segments: Vec<&str> = slug.split('.').collect();
    if segments.is_empty() || segments.len() > 3 {
        return false;
    }
    segments
        .iter()
        .all(|seg| !seg.is_empty() && seg.bytes().all(is_slug_byte))
}

fn is_slug_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'
}

/// Depth is the dot-count + 1 (`level: 1` = top-level, §5), capped at 3.
pub fn slug_depth(slug: &str) -> i16 {
    (slug.matches('.').count() + 1) as i16
}

fn gql_kind_to_kafka(kind: GqlCategoryKind) -> KafkaCategoryKind {
    match kind {
        GqlCategoryKind::Income => KafkaCategoryKind::Income,
        GqlCategoryKind::Expense => KafkaCategoryKind::Expense,
        GqlCategoryKind::Transfer => KafkaCategoryKind::Transfer,
        GqlCategoryKind::Saving => KafkaCategoryKind::Saving,
    }
}

fn gql_kind_str(kind: GqlCategoryKind) -> &'static str {
    match kind {
        GqlCategoryKind::Income => "income",
        GqlCategoryKind::Expense => "expense",
        GqlCategoryKind::Transfer => "transfer",
        GqlCategoryKind::Saving => "saving",
    }
}

pub(crate) fn kafka_kind_to_gql(kind: &str) -> GqlCategoryKind {
    match kind {
        "income" => GqlCategoryKind::Income,
        "expense" => GqlCategoryKind::Expense,
        "transfer" => GqlCategoryKind::Transfer,
        "saving" => GqlCategoryKind::Saving,
        other => {
            tracing::warn!(kind = %other, "unknown category kind in projection, defaulting to EXPENSE");
            GqlCategoryKind::Expense
        }
    }
}

/// `pub(crate)` so `rules.rs`/`labels.rs`/`breakdown.rs` can map a projected
/// `category::Model` without duplicating the enum/field translation.
pub(crate) fn to_graphql(row: category::Model) -> Category {
    Category {
        id: GqlUuid(row.id),
        slug: row.slug,
        name: row.name,
        kind: kafka_kind_to_gql(&row.kind),
        parent_id: row.parent_id.map(GqlUuid),
        depth: row.depth as i32,
        archived: row.archived,
        origin: row.origin,
    }
}

pub async fn fetch_categories(
    db: &DatabaseConnection,
    include_archived: bool,
) -> async_graphql::Result<Vec<Category>> {
    let mut query = category::Entity::find().order_by_asc(category::Column::SortOrder);
    if !include_archived {
        query = query.filter(category::Column::Archived.eq(false));
    }
    let rows = query.all(db).await?;
    Ok(rows.into_iter().map(to_graphql).collect())
}

/// Looks a category up by slug, for parent-kind/parent-archived checks and
/// for resolving `categorySlug`/`parentSlug` inputs elsewhere in WP4.
pub async fn find_by_slug(
    db: &DatabaseConnection,
    slug: &str,
) -> async_graphql::Result<Option<category::Model>> {
    Ok(category::Entity::find()
        .filter(category::Column::Slug.eq(slug))
        .one(db)
        .await?)
}

pub async fn find_by_id(
    db: &DatabaseConnection,
    id: uuid::Uuid,
) -> async_graphql::Result<Option<category::Model>> {
    Ok(category::Entity::find_by_id(id).one(db).await?)
}

/// Upserts the `category` projection, guarded by `revision` (§2.1): a
/// replay of the same or an older record is a no-op.
async fn upsert_category(
    db: &DatabaseConnection,
    record: &CategoryRecord,
) -> async_graphql::Result<()> {
    let kind_str = match record.kind {
        KafkaCategoryKind::Income => "income",
        KafkaCategoryKind::Expense => "expense",
        KafkaCategoryKind::Transfer => "transfer",
        KafkaCategoryKind::Saving => "saving",
    };
    let origin_str = match record.origin {
        CategoryOrigin::Seed => "seed",
        CategoryOrigin::User => "user",
    };
    let parent_id = record
        .parent_slug
        .as_ref()
        .map(|slug| category_uuid(slug));

    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        r#"INSERT INTO category
             (id, slug, parent_id, name, kind, depth, sort_order, archived, origin, owner_user_id, revision)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
           ON CONFLICT (id) DO UPDATE SET
             slug = excluded.slug, parent_id = excluded.parent_id, name = excluded.name,
             kind = excluded.kind, depth = excluded.depth, sort_order = excluded.sort_order,
             archived = excluded.archived, origin = excluded.origin,
             owner_user_id = excluded.owner_user_id, revision = excluded.revision
           WHERE category.revision <= excluded.revision"#,
        vec![
            record.id.into(),
            record.slug.clone().into(),
            parent_id.into(),
            record.name.clone().into(),
            kind_str.into(),
            record.depth.into(),
            record.sort_order.into(),
            record.archived.into(),
            origin_str.into(),
            record.owner_user_id.into(),
            record.revision.into(),
        ],
    );
    db.execute(stmt).await?;
    Ok(())
}

/// `createCategory` (§5): depth > 3, a duplicate slug, a malformed slug, a
/// parent with a different `kind`, or an archived parent are all rejected.
pub async fn create_category(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    input: CategoryInput,
) -> async_graphql::Result<Category> {
    if !is_valid_slug(&input.slug) {
        return Err(validation_error(format!(
            "'{}' is not a valid category slug (expected ^[a-z0-9_]+(\\.[a-z0-9_]+){{0,2}}$)",
            input.slug
        )));
    }
    let depth = slug_depth(&input.slug);
    if depth > 3 {
        return Err(validation_error(format!(
            "'{}' is too deep: categories are at most 3 levels",
            input.slug
        )));
    }
    if find_by_slug(db, &input.slug).await?.is_some() {
        return Err(validation_error(format!(
            "a category with slug '{}' already exists",
            input.slug
        )));
    }

    if let Some(parent_slug) = &input.parent_slug {
        match find_by_slug(db, parent_slug).await? {
            Some(parent) => {
                if parent.archived {
                    return Err(validation_error(format!(
                        "parent category '{parent_slug}' is archived"
                    )));
                }
                if parent.kind != gql_kind_str(input.kind) {
                    return Err(validation_error(format!(
                        "parent category '{parent_slug}' has a different kind"
                    )));
                }
            }
            None => {
                return Err(validation_error(format!(
                    "parent category '{parent_slug}' does not exist"
                )))
            }
        }
    }

    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let id = category_uuid(&input.slug);
    let revision = Utc::now();
    let record = CategoryRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        id,
        slug: input.slug.clone(),
        parent_slug: input.parent_slug.clone(),
        name: input.name.clone(),
        kind: gql_kind_to_kafka(input.kind),
        depth,
        sort_order: 0,
        archived: false,
        origin: CategoryOrigin::User,
        owner_user_id: None,
        revision,
    };

    let value = serde_json::to_vec(&record)
        .map_err(|e| async_graphql::Error::new(format!("failed to serialize category: {e}")))?;
    publish_event(publisher, TOPIC_CATEGORY, &id.to_string(), &value).await?;
    upsert_category(db, &record).await?;

    find_by_id(db, id)
        .await?
        .map(to_graphql)
        .ok_or_else(|| async_graphql::Error::new("category upsert did not take effect"))
}

/// `renameCategory` (§5): `slug` is immutable, only `name` changes.
pub async fn rename_category(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    id: uuid::Uuid,
    name: String,
) -> async_graphql::Result<Category> {
    let existing = find_by_id(db, id)
        .await?
        .ok_or_else(|| async_graphql::Error::new("category not found"))?;

    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let record = CategoryRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        id: existing.id,
        slug: existing.slug.clone(),
        parent_slug: None, // resolved below (needs `existing.parent_id`'s own slug).
        name,
        kind: parse_kind(&existing.kind),
        depth: existing.depth,
        sort_order: existing.sort_order,
        archived: existing.archived,
        origin: parse_origin(&existing.origin),
        owner_user_id: existing.owner_user_id,
        revision: Utc::now(),
    };
    let record = resolve_parent_slug(db, &existing, record).await?;

    let value = serde_json::to_vec(&record)
        .map_err(|e| async_graphql::Error::new(format!("failed to serialize category: {e}")))?;
    publish_event(publisher, TOPIC_CATEGORY, &id.to_string(), &value).await?;
    upsert_category(db, &record).await?;

    find_by_id(db, id)
        .await?
        .map(to_graphql)
        .ok_or_else(|| async_graphql::Error::new("category upsert did not take effect"))
}

/// `archiveCategory` (§5): sets `archived = true`; existing labels keep
/// rendering it, there is no delete.
pub async fn archive_category(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    id: uuid::Uuid,
) -> async_graphql::Result<Category> {
    let existing = find_by_id(db, id)
        .await?
        .ok_or_else(|| async_graphql::Error::new("category not found"))?;

    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let record = CategoryRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        id: existing.id,
        slug: existing.slug.clone(),
        parent_slug: None,
        name: existing.name.clone(),
        kind: parse_kind(&existing.kind),
        depth: existing.depth,
        sort_order: existing.sort_order,
        archived: true,
        origin: parse_origin(&existing.origin),
        owner_user_id: existing.owner_user_id,
        revision: Utc::now(),
    };
    let record = resolve_parent_slug(db, &existing, record).await?;

    let value = serde_json::to_vec(&record)
        .map_err(|e| async_graphql::Error::new(format!("failed to serialize category: {e}")))?;
    publish_event(publisher, TOPIC_CATEGORY, &id.to_string(), &value).await?;
    upsert_category(db, &record).await?;

    find_by_id(db, id)
        .await?
        .map(to_graphql)
        .ok_or_else(|| async_graphql::Error::new("category upsert did not take effect"))
}

async fn resolve_parent_slug(
    db: &DatabaseConnection,
    existing: &category::Model,
    mut record: CategoryRecord,
) -> async_graphql::Result<CategoryRecord> {
    record.parent_slug = match existing.parent_id {
        Some(parent_id) => find_by_id(db, parent_id).await?.map(|p| p.slug),
        None => None,
    };
    Ok(record)
}

fn parse_kind(kind: &str) -> KafkaCategoryKind {
    match kind {
        "income" => KafkaCategoryKind::Income,
        "expense" => KafkaCategoryKind::Expense,
        "transfer" => KafkaCategoryKind::Transfer,
        "saving" => KafkaCategoryKind::Saving,
        other => {
            tracing::warn!(kind = %other, "unknown category kind in projection, defaulting to expense");
            KafkaCategoryKind::Expense
        }
    }
}

fn parse_origin(origin: &str) -> CategoryOrigin {
    match origin {
        "seed" => CategoryOrigin::Seed,
        _ => CategoryOrigin::User,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_slugs_accepted() {
        assert!(is_valid_slug("food"));
        assert!(is_valid_slug("food.groceries"));
        assert!(is_valid_slug("a.b.c"));
        assert!(is_valid_slug("utilities.rundfunk"));
    }

    #[test]
    fn invalid_slugs_rejected() {
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("Food"));
        assert!(!is_valid_slug("food-groceries"));
        assert!(!is_valid_slug("food.groceries.extra.deep"));
        assert!(!is_valid_slug("food."));
        assert!(!is_valid_slug(".food"));
        assert!(!is_valid_slug("food..groceries"));
    }

    #[test]
    fn depth_is_dot_count_plus_one() {
        assert_eq!(slug_depth("food"), 1);
        assert_eq!(slug_depth("food.groceries"), 2);
        assert_eq!(slug_depth("a.b.c"), 3);
    }
}
