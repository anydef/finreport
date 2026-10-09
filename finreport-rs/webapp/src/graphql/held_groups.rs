//! `heldMerchantGroups`: the review queue grouped by merchant.
//!
//! One row per `transaction.counterparty_key` among the caller's held
//! (`transaction_label.status = 'needs_review'`) transactions, computed by a
//! single aggregate statement (`GROUP BY` + `mode()` + one join for the
//! proposal votes), so there is no per-group follow-up query and the cost is
//! one scan of the held rows, not a load of every transaction.
//!
//! The held predicate is the one `TransactionFilter.needsReview` applies
//! (`build_condition`), so assigning a group through `setTransactionsCategory`
//! with `{ needsReview: true, counterpartyKeys: [key] }` touches exactly the
//! rows a group counted. An integration test asserts that equality.
//!
//! Transactions without a usable key (NULL or empty) cannot be grouped. They
//! are collected into ONE bucket whose `counterpartyKey` is `null`, rather
//! than dropped, so the group counts always sum to the held total.

use async_graphql::SimpleObject;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, QueryResult, Statement};
use uuid::Uuid;

use crate::graphql::display_aliases::AliasBook;
use crate::graphql::scalars::Decimal as GqlDecimal;
use crate::graphql::transactions::clamp_limit;
use crate::graphql::types::ReviewReason;

/// Display name of the bucket holding transactions that have no counterparty key.
pub const UNGROUPED_NAME: &str = "No merchant key";

#[derive(SimpleObject, Debug)]
pub struct HeldMerchantGroup {
    /// The normalised merchant key (`transaction.counterparty_key`), the value
    /// to pass in `TransactionFilter.counterpartyKeys`. `null` for the single
    /// bucket of held transactions that have no key; no filter can address
    /// that bucket, only individual transaction ids can.
    pub counterparty_key: Option<String>,
    /// What the caller sees for the merchant: their alias for
    /// `counterpartyKey` when they set one, else `rawName`. The ungrouped
    /// bucket is named "No merchant key" and is never aliased.
    pub display_name: String,
    /// The bank's own name: the most common `counterpartyName` among the
    /// group's held transactions (ties broken alphabetically, so it is
    /// stable), falling back to the key. Shown as secondary detail next to an
    /// alias, as on a transaction row.
    pub raw_name: String,
    /// How many held transactions the group has.
    pub held_count: i32,
    /// Signed sum of the group's amounts (negative = net spending), in
    /// `currency`. A merchant paid in two currencies is summed as-is.
    pub total_amount: GqlDecimal,
    /// The most common currency of the group.
    pub currency: String,
    /// The distinct review reasons present in the group.
    pub review_reasons: Vec<ReviewReason>,
    /// The LLM's most common proposed category path (`proposedCategoryPath`)
    /// among the group, or `null` if none of them carries one. Ties are broken
    /// alphabetically.
    pub proposed_category_path: Option<String>,
    /// How many of the group's held transactions carry exactly
    /// `proposedCategoryPath`. The proposal is unanimous iff this equals
    /// `heldCount`; anything less means the group disagrees (or some members
    /// have no proposal) and a one-click accept would be a guess.
    pub proposed_category_votes: i32,
}

#[derive(SimpleObject, Debug)]
pub struct HeldMerchantGroups {
    /// This page of groups, largest `heldCount` first (ties by name).
    pub groups: Vec<HeldMerchantGroup>,
    /// Number of groups in total, the ungrouped bucket included.
    pub group_count: i32,
    /// Number of held transactions in total; equals the sum of every group's
    /// `heldCount` and `reviewQueue.totalCount`.
    pub held_count: i32,
}

const GROUPS_SQL: &str = "\
WITH held AS ( \
    SELECT NULLIF(t.counterparty_key, '') AS k, t.counterparty_name AS name, t.amount, \
           t.currency, tl.review_reason AS reason, tl.proposed_category_path AS p \
    FROM transaction t \
    JOIN transaction_label tl ON tl.transaction_id = t.id \
    WHERE tl.status = 'needs_review' AND t.account_id = ANY($1) \
), g AS ( \
    SELECT k, \
           mode() WITHIN GROUP (ORDER BY name) AS name, \
           mode() WITHIN GROUP (ORDER BY currency) AS currency, \
           count(*) AS held, \
           sum(amount) AS total, \
           array_remove(array_agg(DISTINCT reason), NULL) AS reasons, \
           mode() WITHIN GROUP (ORDER BY p) AS p \
    FROM held GROUP BY k \
), votes AS ( \
    SELECT k, p, count(*) AS votes FROM held WHERE p IS NOT NULL GROUP BY k, p \
) \
SELECT g.k AS key, COALESCE(g.name, g.k, '') AS name, g.currency, g.held, g.total, \
       g.reasons, g.p AS proposed, COALESCE(v.votes, 0) AS votes \
FROM g LEFT JOIN votes v ON v.k IS NOT DISTINCT FROM g.k AND v.p = g.p \
ORDER BY g.held DESC, COALESCE(g.name, g.k, '') ASC, g.k ASC NULLS LAST \
LIMIT $2 OFFSET $3";

const TOTALS_SQL: &str = "\
SELECT count(*) AS held, \
       count(DISTINCT NULLIF(t.counterparty_key, '')) \
         + CASE WHEN bool_or(NULLIF(t.counterparty_key, '') IS NULL) THEN 1 ELSE 0 END AS groups \
FROM transaction t \
JOIN transaction_label tl ON tl.transaction_id = t.id \
WHERE tl.status = 'needs_review' AND t.account_id = ANY($1)";

fn review_reasons(raw: &[String]) -> Vec<ReviewReason> {
    let mut out = Vec::new();
    for reason in raw {
        let parsed = match reason.as_str() {
            "ambiguous" => ReviewReason::Ambiguous,
            "new_category" => ReviewReason::NewCategory,
            // Other stored reasons have no SDL variant (see `labels.rs`).
            _ => continue,
        };
        if !out.contains(&parsed) {
            out.push(parsed);
        }
    }
    out
}

fn group_from_row(row: &QueryResult, aliases: &AliasBook) -> Result<HeldMerchantGroup, DbErr> {
    let key: Option<String> = row.try_get("", "key")?;
    let name: String = row.try_get("", "name")?;
    let reasons: Vec<String> = row.try_get("", "reasons")?;
    let raw_name = if key.is_none() { UNGROUPED_NAME.to_string() } else { name };
    Ok(HeldMerchantGroup {
        display_name: aliases
            .counterparty_display(key.as_deref(), Some(&raw_name))
            .unwrap_or_else(|| raw_name.clone()),
        raw_name,
        counterparty_key: key,
        held_count: row.try_get::<i64>("", "held")? as i32,
        total_amount: GqlDecimal(row.try_get::<rust_decimal::Decimal>("", "total")?),
        currency: row.try_get("", "currency")?,
        review_reasons: review_reasons(&reasons),
        proposed_category_path: row.try_get("", "proposed")?,
        proposed_category_votes: row.try_get::<i64>("", "votes")? as i32,
    })
}

pub async fn fetch_held_merchant_groups(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    aliases: &AliasBook,
    limit: i32,
    offset: i32,
) -> async_graphql::Result<HeldMerchantGroups> {
    if scoped_ids.is_empty() {
        return Ok(HeldMerchantGroups { groups: vec![], group_count: 0, held_count: 0 });
    }
    let backend = sea_orm::DatabaseBackend::Postgres;
    let rows = db
        .query_all(Statement::from_sql_and_values(
            backend,
            GROUPS_SQL,
            vec![
                scoped_ids.to_vec().into(),
                (clamp_limit(limit) as i64).into(),
                (offset.max(0) as i64).into(),
            ],
        ))
        .await?;
    let groups = rows.iter().map(|r| group_from_row(r, aliases)).collect::<Result<Vec<_>, _>>()?;

    // Totals are their own (single) statement so they stay right for a page
    // past the end, where the groups query returns no rows to carry them.
    let totals = db
        .query_one(Statement::from_sql_and_values(
            backend,
            TOTALS_SQL,
            vec![scoped_ids.to_vec().into()],
        ))
        .await?
        .ok_or_else(|| async_graphql::Error::new("held totals returned no row"))?;
    let held_count = totals.try_get::<i64>("", "held")? as i32;
    let group_count = totals.try_get::<i64>("", "groups")? as i32;
    tracing::debug!(groups = groups.len(), group_count, held_count, "held merchant groups");
    Ok(HeldMerchantGroups { groups, group_count, held_count })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_reasons_dedupe_and_skip_reasons_without_an_sdl_variant() {
        let raw = ["ambiguous", "provider_error", "new_category", "ambiguous"].map(String::from);
        assert_eq!(
            review_reasons(&raw),
            vec![ReviewReason::Ambiguous, ReviewReason::NewCategory]
        );
        assert!(review_reasons(&[]).is_empty());
    }
}
