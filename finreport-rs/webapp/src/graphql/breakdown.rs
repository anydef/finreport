//! `categoryBreakdown` (§5): split-aware category rollup. A transaction
//! with a valid (non-`invalid`) split contributes its parts; otherwise the
//! whole transaction counts once against its `transaction_label` category,
//! or into `uncategorized`/`needsReview` when it has none/is held.

use entity::entities::{category, transaction_label, transaction_split};
use rust_decimal::Decimal as RustDecimal;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::collections::HashMap;
use uuid::Uuid;

use crate::graphql::categories::to_graphql;
use crate::graphql::scalars::{Decimal as GqlDecimal, Uuid as GqlUuid};
use crate::graphql::transactions::build_condition_with_category_filters;
use crate::graphql::types::{
    Category, CategoryBreakdown, CategoryBreakdownRow, CategoryKind as GqlCategoryKind,
    TransactionFilter,
};

/// Deterministic, non-persisted ids for the two sentinel rows — `Category`
/// is non-null on `CategoryBreakdownRow`, but neither "uncategorized" nor
/// "needsReview" is a real row in the `category` table.
const UNCATEGORIZED_ID: Uuid = Uuid::from_bytes([0xFF; 16]);
const NEEDS_REVIEW_ID: Uuid = Uuid::from_bytes([0xFE; 16]);

fn sentinel_category(id: Uuid, slug: &str, name: &str) -> Category {
    Category {
        id: GqlUuid(id),
        slug: slug.to_string(),
        name: name.to_string(),
        kind: GqlCategoryKind::Expense,
        parent_id: None,
        depth: 0,
        archived: false,
        origin: "seed".to_string(),
    }
}

/// One leaf-level contribution before rollup: either a whole transaction or
/// one split part.
struct Contribution {
    transaction_id: Uuid,
    category_id: Option<Uuid>,
    amount: RustDecimal,
}

pub async fn fetch_breakdown(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
    level: i32,
    kind: Option<GqlCategoryKind>,
) -> async_graphql::Result<CategoryBreakdown> {
    let level = level.max(1) as i16;

    if scoped_ids.is_empty() {
        return Ok(CategoryBreakdown {
            rows: vec![],
            uncategorized: None,
            needs_review: None,
            currency: "EUR".to_string(),
        });
    }

    let condition = build_condition_with_category_filters(db, scoped_ids, filter).await?;
    let transaction_rows = entity::entities::transaction::Entity::find()
        .filter(condition)
        .all(db)
        .await?;
    let currency = transaction_rows
        .first()
        .map(|t| t.currency.clone())
        .unwrap_or_else(|| "EUR".to_string());
    let transaction_ids: Vec<Uuid> = transaction_rows.iter().map(|t| t.id).collect();
    let amounts_by_id: HashMap<Uuid, RustDecimal> =
        transaction_rows.iter().map(|t| (t.id, t.amount)).collect();

    if transaction_ids.is_empty() {
        return Ok(CategoryBreakdown {
            rows: vec![],
            uncategorized: None,
            needs_review: None,
            currency,
        });
    }

    let label_rows = transaction_label::Entity::find()
        .filter(transaction_label::Column::TransactionId.is_in(transaction_ids.clone()))
        .all(db)
        .await?;
    let labels_by_transaction: HashMap<Uuid, &transaction_label::Model> =
        label_rows.iter().map(|r| (r.transaction_id, r)).collect();

    let split_rows = transaction_split::Entity::find()
        .filter(transaction_split::Column::TransactionId.is_in(transaction_ids.clone()))
        .filter(transaction_split::Column::Invalid.eq(false))
        .all(db)
        .await?;
    let mut splits_by_transaction: HashMap<Uuid, Vec<&transaction_split::Model>> = HashMap::new();
    for row in &split_rows {
        splits_by_transaction.entry(row.transaction_id).or_default().push(row);
    }

    // Every category that could be reached by a label/split id or one of
    // its ancestors, loaded once (§9 N+1 avoidance) — the table is small
    // (≤ 3 levels deep) so this is simpler and no less correct than N
    // per-row ancestor-chain queries.
    let all_categories = category::Entity::find().all(db).await?;
    let categories_by_id: HashMap<Uuid, category::Model> =
        all_categories.into_iter().map(|c| (c.id, c)).collect();

    let mut needs_review_total = RustDecimal::ZERO;
    let mut needs_review_count = 0i32;
    let mut uncategorized_total = RustDecimal::ZERO;
    let mut uncategorized_count = 0i32;

    // Rolled-up category id -> (amount, set of contributing transaction ids).
    let mut rolled: HashMap<Uuid, (RustDecimal, std::collections::HashSet<Uuid>)> = HashMap::new();

    for &transaction_id in &transaction_ids {
        let whole_amount = amounts_by_id.get(&transaction_id).copied().unwrap_or(RustDecimal::ZERO);
        let contributions: Vec<Contribution> = match splits_by_transaction.get(&transaction_id) {
            Some(parts) if !parts.is_empty() => parts
                .iter()
                .map(|p| Contribution {
                    transaction_id,
                    category_id: Some(p.category_id),
                    amount: p.amount,
                })
                .collect(),
            _ => {
                let label = labels_by_transaction.get(&transaction_id);
                match label {
                    Some(l) if l.status == "needs_review" => {
                        needs_review_total += whole_amount.abs();
                        needs_review_count += 1;
                        continue;
                    }
                    Some(l) => vec![Contribution {
                        transaction_id,
                        category_id: l.category_id,
                        amount: whole_amount,
                    }],
                    None => {
                        uncategorized_total += whole_amount.abs();
                        uncategorized_count += 1;
                        continue;
                    }
                }
            }
        };

        for contribution in contributions {
            let Some(category_id) = contribution.category_id else {
                uncategorized_total += contribution.amount.abs();
                uncategorized_count += 1;
                continue;
            };
            let rolled_id = roll_up(&categories_by_id, category_id, level);
            let entry = rolled.entry(rolled_id).or_insert_with(|| (RustDecimal::ZERO, std::collections::HashSet::new()));
            entry.0 += contribution.amount.abs();
            entry.1.insert(contribution.transaction_id);
        }
    }

    // Kind totals for `share` (§5): computed per-kind over the rolled rows
    // only — `uncategorized`/`needsReview` have no assigned kind and are
    // reported separately, never diluting a kind's percentage base.
    let mut kind_totals: HashMap<GqlCategoryKind, RustDecimal> = HashMap::new();
    for (&category_id, (amount, _)) in &rolled {
        if let Some(c) = categories_by_id.get(&category_id) {
            let k = crate::graphql::categories::kafka_kind_to_gql(&c.kind);
            *kind_totals.entry(k).or_insert(RustDecimal::ZERO) += *amount;
        }
    }

    let mut rows: Vec<CategoryBreakdownRow> = rolled
        .into_iter()
        .filter_map(|(category_id, (amount, transactions))| {
            let category_row = categories_by_id.get(&category_id)?;
            let category_kind = crate::graphql::categories::kafka_kind_to_gql(&category_row.kind);
            if let Some(requested_kind) = kind
                && category_kind != requested_kind
            {
                return None;
            }
            let kind_total = kind_totals.get(&category_kind).copied().unwrap_or(RustDecimal::ZERO);
            let share = if kind_total.is_zero() {
                0.0
            } else {
                use rust_decimal::prelude::ToPrimitive;
                (amount / kind_total).to_f32().unwrap_or(0.0)
            };
            Some(CategoryBreakdownRow {
                category: to_graphql(category_row.clone()),
                amount: GqlDecimal(amount),
                transaction_count: transactions.len() as i32,
                share,
            })
        })
        .collect();
    rows.sort_by(|a, b| a.category.slug.cmp(&b.category.slug));

    let uncategorized = (uncategorized_count > 0).then(|| CategoryBreakdownRow {
        category: sentinel_category(UNCATEGORIZED_ID, "uncategorized", "Uncategorized"),
        amount: GqlDecimal(uncategorized_total),
        transaction_count: uncategorized_count,
        share: 0.0,
    });
    let needs_review = (needs_review_count > 0).then(|| CategoryBreakdownRow {
        category: sentinel_category(NEEDS_REVIEW_ID, "needs_review", "Needs Review"),
        amount: GqlDecimal(needs_review_total),
        transaction_count: needs_review_count,
        share: 0.0,
    });

    Ok(CategoryBreakdown {
        rows,
        uncategorized,
        needs_review,
        currency,
    })
}

/// Walks `category_id`'s ancestor chain up to `level` (`1` = top-level). A
/// category shallower than or at `level` already (e.g. a top-level category
/// requested at `level: 2`) is returned unchanged — there is no deeper
/// descendant to roll up from.
fn roll_up(categories: &HashMap<Uuid, category::Model>, category_id: Uuid, level: i16) -> Uuid {
    let mut current = category_id;
    loop {
        let Some(row) = categories.get(&current) else {
            return current;
        };
        if row.depth <= level {
            return current;
        }
        match row.parent_id {
            Some(parent_id) => current = parent_id,
            None => return current,
        }
    }
}

