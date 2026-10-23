//! Turning an `Apply` Amazon match into real transaction splits, and back.
//!
//! **This is the one place in the Amazon pipeline that changes the user's own
//! data.** Everything before it only stored evidence. The design leans on three
//! rules, each enforced by a pure function below so it is unit-tested without a
//! database:
//!
//! 1. *A user's decision always wins.* [`decide`] applies only to a transaction
//!    with no splits and no pinned category. A manual split, or a category the
//!    user set, is reported and skipped - never replaced. (`splitTransaction`
//!    itself would overwrite both, which is why this check lives here.)
//! 2. *Idempotent.* A transaction whose live split equals what applying would
//!    write is [`Decision::AlreadyApplied`], and a different outcome for an
//!    already-applied charge is refused, not stacked.
//! 3. *Reversible, and only our own.* Every application is recorded in
//!    `purchase_order_split` with the parts it wrote. [`undo_decision`] removes
//!    a split only while it still equals that record; if the user has touched
//!    it since, it is left exactly as the user has it.
//!
//! **What a part carries.** A split part in this system is an amount and a
//! category - there is no description or tag column - and categorising item
//! titles is not built. So the *caller chooses the category for each item*
//! (and for the shipping-and-adjustments remainder) when applying; the server
//! computes every amount itself and never trusts one from the client. The item
//! title is kept per part in the application record, not on the part.

use amazon_orders::{ChargePlan, MatchStatus, Order};
use async_graphql::{Enum, InputObject, SimpleObject};
use chrono::Utc;
use entity::entities::{purchase_order_split, transaction, transaction_split, transaction_user_label};
use rust_decimal::Decimal;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Statement,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::graphql::categories::find_by_slug;
use crate::graphql::current_user::AuthenticatedUser;
use crate::graphql::labels;
use crate::graphql::purchase_orders::{load_match_inputs, validation_error};
use crate::graphql::scalars::{Decimal as GqlDecimal, Uuid as GqlUuid};
use crate::graphql::types::SplitPartInput;
use crate::kafka::producer::EventPublisher;
use crate::kafka::purchase_orders::SOURCE_AMAZON;

/// The label of the part that absorbs shipping, tax, gift cards and promotions.
pub const REMAINDER_LABEL: &str = "Shipping & adjustments";

// ---------------------------------------------------------------------------
// Pure: building the parts
// ---------------------------------------------------------------------------

/// One part before its category is resolved to an id. `amount` is signed like
/// the transaction it splits.
#[derive(Debug, Clone, PartialEq)]
pub struct PartDraft {
    pub amount: Decimal,
    pub category_slug: String,
    /// Item title (with quantity) or [`REMAINDER_LABEL`].
    pub label: String,
}

/// Why a charge cannot be turned into parts. Each has user-facing wording.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    CurrencyMismatch { order: String, charge: String },
    AmountMismatch,
    RemainderExceedsCharge,
    NothingToSplit,
    MissingCategory { item: String },
    MissingRemainderCategory,
    PartSign,
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::CurrencyMismatch { order, charge } => {
                format!("the order is in {order} but the charge is in {charge}")
            }
            Refusal::AmountMismatch => "the charge no longer equals the amount the match was made on".into(),
            Refusal::RemainderExceedsCharge => {
                "the items add up to more than the charge, so the parts cannot sum to it".into()
            }
            Refusal::NothingToSplit => {
                "there is nothing to split: the charge is a single item with no shipping or adjustments".into()
            }
            Refusal::MissingCategory { item } => format!("no category was chosen for \"{item}\""),
            Refusal::MissingRemainderCategory => "no category was chosen for shipping & adjustments".into(),
            Refusal::PartSign => "an item amount has the wrong sign for this charge".into(),
        }
    }
}

fn item_label(order: &Order, item_index: usize) -> String {
    let item = &order.items[item_index];
    if item.quantity > 1 {
        format!("{} (x{})", item.title, item.quantity)
    } else {
        item.title.clone()
    }
}

/// Builds the parts for one charge from the matcher's own plan.
///
/// The amounts come from `plan` (the matcher already proved they reconcile);
/// the remainder is an explicit, visible part, never spread across items
/// (spec 2.4). The function re-checks the sum itself: parts must equal the
/// transaction amount exactly, in `Decimal`, or nothing is produced.
///
/// `item_categories` is indexed by the order's item index; an empty slug counts
/// as not chosen. Zero-amount items produce no part (and need no category).
pub fn build_parts(
    order: &Order,
    plan: &ChargePlan,
    tx_amount: Decimal,
    tx_currency: &str,
    item_categories: &[String],
    remainder_category: Option<&str>,
) -> Result<Vec<PartDraft>, Refusal> {
    if !order.currency.eq_ignore_ascii_case(tx_currency) {
        return Err(Refusal::CurrencyMismatch { order: order.currency.clone(), charge: tx_currency.to_string() });
    }
    // The plan's `charge` is positive; the transaction is a debit.
    if tx_amount.is_sign_positive() || tx_amount != -plan.charge {
        return Err(Refusal::AmountMismatch);
    }
    if plan.remainder.is_sign_negative() && !plan.remainder.is_zero() {
        return Err(Refusal::RemainderExceedsCharge);
    }

    let mut drafts = Vec::new();
    for part in &plan.parts {
        if part.amount.is_zero() {
            continue;
        }
        if part.amount.is_sign_negative() {
            return Err(Refusal::PartSign);
        }
        let slug = item_categories
            .get(part.item_index)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Refusal::MissingCategory { item: order.items[part.item_index].title.clone() })?;
        drafts.push(PartDraft {
            amount: -part.amount,
            category_slug: slug.to_string(),
            label: item_label(order, part.item_index),
        });
    }
    if !plan.remainder.is_zero() {
        let slug = remainder_category
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or(Refusal::MissingRemainderCategory)?;
        drafts.push(PartDraft {
            amount: -plan.remainder,
            category_slug: slug.to_string(),
            label: REMAINDER_LABEL.to_string(),
        });
    }

    if drafts.len() < 2 {
        return Err(Refusal::NothingToSplit);
    }
    let sum: Decimal = drafts.iter().map(|d| d.amount).sum();
    if sum != tx_amount {
        return Err(Refusal::AmountMismatch);
    }
    Ok(drafts)
}

// ---------------------------------------------------------------------------
// Pure: should we write, and may we undo
// ---------------------------------------------------------------------------

/// A part as stored in `purchase_order_split.parts` and as written to
/// `transaction_split`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredPart {
    pub index: i32,
    pub amount: Decimal,
    pub category_id: Uuid,
    pub category_slug: String,
    pub label: String,
}

/// The live split rows of a transaction: `(part_index, amount, category_id)`.
pub type LiveSplit = Vec<(i32, Decimal, Uuid)>;

/// What is on a transaction right now, as far as applying cares.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CurrentState {
    pub splits: LiveSplit,
    /// The user pinned a category (`transaction_user_label.category_id`).
    pub category_override: bool,
}

fn normalised(mut live: LiveSplit) -> LiveSplit {
    live.sort_by_key(|(index, _, _)| *index);
    live
}

fn stored_as_live(stored: &[StoredPart]) -> LiveSplit {
    normalised(stored.iter().map(|p| (p.index, p.amount, p.category_id)).collect())
}

/// True while the live split is exactly what we recorded writing.
pub fn split_matches_record(stored: &[StoredPart], live: &LiveSplit) -> bool {
    !live.is_empty() && stored_as_live(stored) == normalised(live.clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The transaction already has splits that are not ours.
    UserSplit,
    /// The user pinned a category on it.
    UserCategory,
    /// We applied this before and the result would differ; undo first.
    AppliedDifferently,
}

impl SkipReason {
    pub fn message(self) -> &'static str {
        match self {
            SkipReason::UserSplit => "it already has a split that was not made from an order; left as it is",
            SkipReason::UserCategory => "you set a category on it; left as it is (clear it first to split)",
            SkipReason::AppliedDifferently => {
                "this order is already applied with different amounts or categories; undo it first to change that"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Apply,
    AlreadyApplied,
    Skip(SkipReason),
}

/// Whether writing `wanted` to a transaction is safe, given what is on it and
/// what we recorded applying to it before (`record`).
pub fn decide(state: &CurrentState, record: Option<&[StoredPart]>, wanted: &[StoredPart]) -> Decision {
    if !state.splits.is_empty() {
        return match record {
            Some(stored) if split_matches_record(stored, &state.splits) => {
                if stored_as_live(stored) == stored_as_live(wanted) {
                    Decision::AlreadyApplied
                } else {
                    Decision::Skip(SkipReason::AppliedDifferently)
                }
            }
            _ => Decision::Skip(SkipReason::UserSplit),
        };
    }
    if state.category_override {
        return Decision::Skip(SkipReason::UserCategory);
    }
    // No splits and no override: free to write. A leftover record (the split
    // was removed by hand) is simply replaced.
    Decision::Apply
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndoDecision {
    /// The split is still exactly ours: remove it.
    Undo,
    /// The user has changed or removed it since; touch nothing.
    LeaveAlone,
}

pub fn undo_decision(state: &CurrentState, record: &[StoredPart]) -> UndoDecision {
    if split_matches_record(record, &state.splits) {
        UndoDecision::Undo
    } else {
        UndoDecision::LeaveAlone
    }
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

#[derive(InputObject, Clone, Debug)]
pub struct ApplyPurchaseOrderInput {
    pub order_id: String,
    /// One category slug per item of the order, in the order's item order.
    /// Items that contribute nothing to a charge may be left empty.
    pub item_categories: Vec<String>,
    /// Category for the shipping-and-adjustments part. Required when a charge
    /// has a non-zero remainder.
    pub remainder_category: Option<String>,
}

#[derive(SimpleObject, Clone, Debug, PartialEq)]
pub struct AppliedPart {
    /// Signed like the transaction.
    pub amount: GqlDecimal,
    pub category_slug: String,
    /// Item title, or "Shipping & adjustments".
    pub label: String,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ChargeOutcomeStatus {
    /// The split was written now.
    Applied,
    /// Already applied identically; nothing changed.
    AlreadyApplied,
    /// Not applied; `reason` says why. The transaction is untouched.
    Skipped,
    /// The split we made was removed.
    Undone,
    /// Undo found the split changed since; it was left as the user has it.
    LeftAlone,
}

#[derive(SimpleObject, Clone, Debug, PartialEq)]
pub struct ChargeOutcome {
    pub transaction_id: GqlUuid,
    pub status: ChargeOutcomeStatus,
    pub reason: Option<String>,
    pub parts: Vec<AppliedPart>,
}

#[derive(SimpleObject, Clone, Debug, PartialEq)]
pub struct PurchaseOrderApplyResult {
    pub order_id: String,
    pub outcomes: Vec<ChargeOutcome>,
}

/// An application on record, for the review page.
#[derive(SimpleObject, Clone, Debug, PartialEq)]
pub struct PurchaseOrderApplication {
    pub transaction_id: GqlUuid,
    pub applied_at: crate::graphql::scalars::DateTime,
    pub parts: Vec<AppliedPart>,
    /// The live split still equals what was applied, so undo will remove it.
    /// `false` means it was changed or removed since; undo will not touch it.
    pub intact: bool,
}

pub fn to_applied_parts(parts: &[StoredPart]) -> Vec<AppliedPart> {
    parts
        .iter()
        .map(|p| AppliedPart {
            amount: GqlDecimal(p.amount),
            category_slug: p.category_slug.clone(),
            label: p.label.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Database glue
// ---------------------------------------------------------------------------

fn parse_record(row: &purchase_order_split::Model) -> Vec<StoredPart> {
    serde_json::from_value(row.parts.clone()).unwrap_or_default()
}

async fn current_state(db: &DatabaseConnection, transaction_id: Uuid) -> Result<CurrentState, sea_orm::DbErr> {
    let splits = transaction_split::Entity::find()
        .filter(transaction_split::Column::TransactionId.eq(transaction_id))
        .all(db)
        .await?
        .into_iter()
        .map(|s| (s.part_index, s.amount, s.category_id))
        .collect();
    let category_override = transaction_user_label::Entity::find_by_id(transaction_id)
        .one(db)
        .await?
        .is_some_and(|l| l.category_id.is_some());
    Ok(CurrentState { splits, category_override })
}

/// Live splits for many transactions at once (for the report's `intact`).
pub async fn live_splits(
    db: &DatabaseConnection,
    transaction_ids: &[Uuid],
) -> Result<HashMap<Uuid, LiveSplit>, sea_orm::DbErr> {
    if transaction_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut out: HashMap<Uuid, LiveSplit> = HashMap::new();
    for s in transaction_split::Entity::find()
        .filter(transaction_split::Column::TransactionId.is_in(transaction_ids.to_vec()))
        .all(db)
        .await?
    {
        out.entry(s.transaction_id).or_default().push((s.part_index, s.amount, s.category_id));
    }
    Ok(out)
}

/// All applications on record for the user, keyed by transaction.
pub async fn applications_for_user(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<Vec<purchase_order_split::Model>, sea_orm::DbErr> {
    purchase_order_split::Entity::find()
        .filter(purchase_order_split::Column::UserId.eq(user_id))
        .filter(purchase_order_split::Column::Source.eq(SOURCE_AMAZON))
        .all(db)
        .await
}

/// Claims the transaction for an application. `false` when another apply got
/// there first (the primary key is the lock).
async fn claim(
    db: &DatabaseConnection,
    user_id: Uuid,
    transaction_id: Uuid,
    order_id: &str,
    parts: &[StoredPart],
) -> Result<bool, sea_orm::DbErr> {
    let json = serde_json::to_value(parts).unwrap_or_default();
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        r#"INSERT INTO purchase_order_split (transaction_id, user_id, source, order_id, parts, applied_at)
           VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (transaction_id) DO NOTHING"#,
        vec![
            transaction_id.into(),
            user_id.into(),
            SOURCE_AMAZON.into(),
            order_id.into(),
            json.into(),
            Utc::now().into(),
        ],
    );
    Ok(db.execute(stmt).await?.rows_affected() > 0)
}

async fn release(db: &DatabaseConnection, transaction_id: Uuid) -> Result<(), sea_orm::DbErr> {
    purchase_order_split::Entity::delete_by_id(transaction_id).exec(db).await?;
    Ok(())
}

fn skipped(transaction_id: Uuid, reason: impl Into<String>) -> ChargeOutcome {
    ChargeOutcome {
        transaction_id: GqlUuid(transaction_id),
        status: ChargeOutcomeStatus::Skipped,
        reason: Some(reason.into()),
        parts: vec![],
    }
}

/// `applyPurchaseOrderSplit`: re-runs the matcher server-side, and for an order
/// that is still `Apply` writes the split of each of its charges the caller
/// owns. Nothing the client sends but category slugs is used.
pub async fn apply_purchase_order(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    input: ApplyPurchaseOrderInput,
) -> async_graphql::Result<PurchaseOrderApplyResult> {
    let inputs = load_match_inputs(db, user).await?;
    let order = inputs
        .orders
        .iter()
        .find(|o| o.order_id == input.order_id)
        .ok_or_else(|| validation_error("no such order"))?;
    let m = inputs
        .report
        .matches
        .iter()
        .find(|m| m.order_id == input.order_id)
        .ok_or_else(|| validation_error("this order has no matching bank charge"))?;
    match (&m.status, &m.split) {
        (MatchStatus::Apply, Some(_)) => {}
        _ => return Err(validation_error("this order needs review and cannot be applied")),
    }
    let plans = m.split.as_deref().unwrap_or_default();

    // Resolve every slug the caller named before touching anything.
    let mut category_ids: HashMap<String, Uuid> = HashMap::new();
    for slug in input.item_categories.iter().chain(input.remainder_category.iter()) {
        let slug = slug.trim();
        if slug.is_empty() || category_ids.contains_key(slug) {
            continue;
        }
        let category = find_by_slug(db, slug)
            .await?
            .ok_or_else(|| validation_error(format!("category '{slug}' does not exist")))?;
        category_ids.insert(slug.to_string(), category.id);
    }

    let mut outcomes = Vec::with_capacity(plans.len());
    for plan in plans {
        let Some(tx) = inputs.transactions.get(&plan.transaction_id) else {
            continue;
        };
        // Belt and braces: the candidates were loaded from the caller's own
        // accounts, and `split_transaction` scopes by them again.
        if !user.account_ids.contains(&tx.account_id) {
            outcomes.push(skipped(tx.id, "not one of your transactions"));
            continue;
        }
        outcomes.push(
            apply_one(db, publisher, user, order, plan, tx, &input, &category_ids).await?,
        );
    }
    Ok(PurchaseOrderApplyResult { order_id: input.order_id, outcomes })
}

#[allow(clippy::too_many_arguments)]
async fn apply_one(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    order: &Order,
    plan: &ChargePlan,
    tx: &transaction::Model,
    input: &ApplyPurchaseOrderInput,
    category_ids: &HashMap<String, Uuid>,
) -> async_graphql::Result<ChargeOutcome> {
    let drafts = match build_parts(
        order,
        plan,
        tx.amount,
        &tx.currency,
        &input.item_categories,
        input.remainder_category.as_deref(),
    ) {
        Ok(d) => d,
        Err(refusal) => return Ok(skipped(tx.id, refusal.message())),
    };
    let wanted: Vec<StoredPart> = drafts
        .iter()
        .enumerate()
        .map(|(index, d)| StoredPart {
            index: index as i32,
            amount: d.amount,
            category_id: category_ids[d.category_slug.trim()],
            category_slug: d.category_slug.trim().to_string(),
            label: d.label.clone(),
        })
        .collect();

    let state = current_state(db, tx.id).await?;
    let record_row = purchase_order_split::Entity::find_by_id(tx.id).one(db).await?;
    let record = record_row.as_ref().map(parse_record);
    match decide(&state, record.as_deref(), &wanted) {
        Decision::AlreadyApplied => {
            return Ok(ChargeOutcome {
                transaction_id: GqlUuid(tx.id),
                status: ChargeOutcomeStatus::AlreadyApplied,
                reason: None,
                parts: to_applied_parts(&wanted),
            })
        }
        Decision::Skip(reason) => return Ok(skipped(tx.id, reason.message())),
        Decision::Apply => {}
    }

    // A leftover record whose split is gone is stale; drop it, then claim.
    if record_row.is_some() {
        release(db, tx.id).await?;
    }
    if !claim(db, user.user_id, tx.id, &order.order_id, &wanted).await? {
        return Ok(skipped(tx.id, "another apply of this charge is in progress"));
    }
    let parts = wanted
        .iter()
        .map(|p| SplitPartInput { amount: GqlDecimal(p.amount), category_slug: p.category_slug.clone() })
        .collect();
    if let Err(e) = labels::split_transaction(db, publisher, &user.account_ids, tx.id, parts).await {
        release(db, tx.id).await?;
        return Err(e);
    }
    // `split_transaction` writes nothing when a newer label already won.
    if !split_matches_record(&wanted, &current_state(db, tx.id).await?.splits) {
        release(db, tx.id).await?;
        return Ok(skipped(tx.id, "a newer change to this transaction won; nothing was applied"));
    }
    Ok(ChargeOutcome {
        transaction_id: GqlUuid(tx.id),
        status: ChargeOutcomeStatus::Applied,
        reason: None,
        parts: to_applied_parts(&wanted),
    })
}

/// `undoPurchaseOrderSplit`: removes the splits this order's application wrote
/// - and only those still exactly as written.
pub async fn undo_purchase_order(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    order_id: String,
) -> async_graphql::Result<PurchaseOrderApplyResult> {
    let rows: Vec<purchase_order_split::Model> = applications_for_user(db, user.user_id)
        .await?
        .into_iter()
        .filter(|r| r.order_id == order_id)
        .collect();
    let mut outcomes = Vec::with_capacity(rows.len());
    for row in rows {
        let record = parse_record(&row);
        if !user.account_ids.is_empty()
            && transaction::Entity::find_by_id(row.transaction_id)
                .filter(transaction::Column::AccountId.is_in(user.account_ids.clone()))
                .one(db)
                .await?
                .is_none()
        {
            outcomes.push(skipped(row.transaction_id, "not one of your transactions"));
            continue;
        }
        let state = current_state(db, row.transaction_id).await?;
        let outcome = match undo_decision(&state, &record) {
            UndoDecision::Undo => {
                labels::unsplit_transaction(db, publisher, &user.account_ids, row.transaction_id).await?;
                release(db, row.transaction_id).await?;
                ChargeOutcome {
                    transaction_id: GqlUuid(row.transaction_id),
                    status: ChargeOutcomeStatus::Undone,
                    reason: None,
                    parts: to_applied_parts(&record),
                }
            }
            UndoDecision::LeaveAlone => {
                // The record no longer describes anything; forget it.
                release(db, row.transaction_id).await?;
                ChargeOutcome {
                    transaction_id: GqlUuid(row.transaction_id),
                    status: ChargeOutcomeStatus::LeftAlone,
                    reason: Some("the split was changed or removed since it was applied; left as it is".into()),
                    parts: vec![],
                }
            }
        };
        outcomes.push(outcome);
    }
    Ok(PurchaseOrderApplyResult { order_id, outcomes })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use amazon_orders::{Item, PlannedPart, PriceBasis};
    use chrono::NaiveDate;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn item(title: &str, line: &str, qty: u32) -> Item {
        Item {
            asin: "B0".into(),
            quantity: qty,
            price: d(line),
            discount: None,
            promotion: None,
            promotion_amount: None,
            title: title.into(),
            status: "Delivered".into(),
            returned: false,
            line_total: d(line),
        }
    }

    fn order(items: Vec<Item>, total: &str) -> Order {
        let sum: Decimal = items.iter().map(|i| i.line_total).sum();
        Order {
            order_id: "304-1111111-2222222".into(),
            order_date: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            total: d(total),
            currency: "EUR".into(),
            items,
            price_basis: PriceBasis::LineTotal,
            discounts_netted: false,
            remainder: d(total) - sum,
        }
    }

    fn plan(parts: &[(usize, &str)], charge: &str) -> ChargePlan {
        let parts: Vec<PlannedPart> =
            parts.iter().map(|(i, a)| PlannedPart { item_index: *i, amount: d(a) }).collect();
        let remainder = d(charge) - parts.iter().map(|p| p.amount).sum::<Decimal>();
        ChargePlan { transaction_id: "t".into(), charge: d(charge), parts, remainder }
    }

    fn cats(slugs: &[&str]) -> Vec<String> {
        slugs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parts_are_signed_like_the_charge_and_sum_to_it_exactly() {
        let o = order(vec![item("Anker", "13.60", 1), item("Kabel", "8.99", 1)], "24.09");
        let p = plan(&[(0, "13.60"), (1, "8.99")], "24.09");
        let parts = build_parts(&o, &p, d("-24.09"), "EUR", &cats(&["a", "b"]), Some("fees")).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].amount, d("-13.60"));
        assert_eq!(parts[2].label, REMAINDER_LABEL);
        assert_eq!(parts[2].amount, d("-1.50"));
        assert_eq!(parts.iter().map(|p| p.amount).sum::<Decimal>(), d("-24.09"));
    }

    #[test]
    fn cent_sums_that_floats_get_wrong_are_exact() {
        let o = order(vec![item("a", "0.10", 1), item("b", "0.20", 1)], "0.30");
        let p = plan(&[(0, "0.10"), (1, "0.20")], "0.30");
        let parts = build_parts(&o, &p, d("-0.30"), "EUR", &cats(&["x", "y"]), None).unwrap();
        assert_eq!(parts.iter().map(|p| p.amount).sum::<Decimal>(), d("-0.30"));
        assert_eq!(parts.len(), 2, "no remainder part when there is no remainder");
    }

    #[test]
    fn a_single_item_with_no_remainder_is_not_a_split() {
        let o = order(vec![item("only", "9.99", 1)], "9.99");
        let p = plan(&[(0, "9.99")], "9.99");
        assert_eq!(
            build_parts(&o, &p, d("-9.99"), "EUR", &cats(&["x"]), None),
            Err(Refusal::NothingToSplit)
        );
    }

    #[test]
    fn a_missing_category_or_remainder_category_is_refused() {
        let o = order(vec![item("a", "5.00", 1), item("b", "5.00", 1)], "11.00");
        let p = plan(&[(0, "5.00"), (1, "5.00")], "11.00");
        assert!(matches!(
            build_parts(&o, &p, d("-11.00"), "EUR", &cats(&["x", ""]), Some("r")),
            Err(Refusal::MissingCategory { .. })
        ));
        assert_eq!(
            build_parts(&o, &p, d("-11.00"), "EUR", &cats(&["x", "y"]), None),
            Err(Refusal::MissingRemainderCategory)
        );
    }

    #[test]
    fn a_changed_charge_currency_or_oversized_items_are_refused() {
        let o = order(vec![item("a", "5.00", 1), item("b", "5.00", 1)], "10.00");
        let p = plan(&[(0, "5.00"), (1, "5.00")], "10.00");
        let c = cats(&["x", "y"]);
        assert_eq!(build_parts(&o, &p, d("-10.01"), "EUR", &c, None), Err(Refusal::AmountMismatch));
        assert_eq!(build_parts(&o, &p, d("10.00"), "EUR", &c, None), Err(Refusal::AmountMismatch));
        assert!(matches!(build_parts(&o, &p, d("-10.00"), "USD", &c, None), Err(Refusal::CurrencyMismatch { .. })));
        let over = plan(&[(0, "5.00"), (1, "5.00")], "9.00");
        assert_eq!(build_parts(&o, &over, d("-9.00"), "EUR", &c, None), Err(Refusal::RemainderExceedsCharge));
    }

    #[test]
    fn zero_amount_items_make_no_part_and_need_no_category() {
        let o = order(vec![item("free", "0.00", 1), item("a", "5.00", 1)], "7.00");
        let p = plan(&[(0, "0.00"), (1, "5.00")], "7.00");
        let parts = build_parts(&o, &p, d("-7.00"), "EUR", &cats(&["", "x"]), Some("r")).unwrap();
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn quantity_shows_in_the_label() {
        let o = order(vec![item("Batterien", "6.00", 3), item("b", "1.00", 1)], "7.00");
        let p = plan(&[(0, "6.00"), (1, "1.00")], "7.00");
        let parts = build_parts(&o, &p, d("-7.00"), "EUR", &cats(&["x", "y"]), None).unwrap();
        assert_eq!(parts[0].label, "Batterien (x3)");
    }

    fn sp(index: i32, amount: &str, cat: u128) -> StoredPart {
        StoredPart {
            index,
            amount: d(amount),
            category_id: Uuid::from_u128(cat),
            category_slug: format!("c{cat}"),
            label: "x".into(),
        }
    }

    fn live(parts: &[StoredPart]) -> LiveSplit {
        parts.iter().map(|p| (p.index, p.amount, p.category_id)).collect()
    }

    #[test]
    fn a_clean_transaction_is_applied() {
        let wanted = [sp(0, "-5.00", 1), sp(1, "-5.00", 2)];
        assert_eq!(decide(&CurrentState::default(), None, &wanted), Decision::Apply);
    }

    #[test]
    fn a_users_split_is_never_overwritten() {
        let wanted = [sp(0, "-5.00", 1), sp(1, "-5.00", 2)];
        let manual = CurrentState { splits: live(&[sp(0, "-3.00", 9), sp(1, "-7.00", 9)]), category_override: false };
        assert_eq!(decide(&manual, None, &wanted), Decision::Skip(SkipReason::UserSplit));
        // Even one with identical amounts and categories, if we did not record it.
        let lookalike = CurrentState { splits: live(&wanted), category_override: false };
        assert_eq!(decide(&lookalike, None, &wanted), Decision::Skip(SkipReason::UserSplit));
    }

    #[test]
    fn a_users_category_is_never_overwritten() {
        let wanted = [sp(0, "-5.00", 1), sp(1, "-5.00", 2)];
        let pinned = CurrentState { splits: vec![], category_override: true };
        assert_eq!(decide(&pinned, None, &wanted), Decision::Skip(SkipReason::UserCategory));
    }

    #[test]
    fn applying_twice_is_a_no_op_and_a_different_second_apply_is_refused() {
        let wanted = [sp(0, "-5.00", 1), sp(1, "-5.00", 2)];
        let applied = CurrentState { splits: live(&wanted), category_override: false };
        assert_eq!(decide(&applied, Some(&wanted), &wanted), Decision::AlreadyApplied);
        let other = [sp(0, "-5.00", 1), sp(1, "-5.00", 3)];
        assert_eq!(decide(&applied, Some(&wanted), &other), Decision::Skip(SkipReason::AppliedDifferently));
    }

    #[test]
    fn a_stale_record_does_not_block_a_transaction_the_user_cleared() {
        let wanted = [sp(0, "-5.00", 1), sp(1, "-5.00", 2)];
        assert_eq!(decide(&CurrentState::default(), Some(&wanted), &wanted), Decision::Apply);
    }

    #[test]
    fn a_split_the_user_replaced_after_we_applied_is_theirs() {
        let ours = [sp(0, "-5.00", 1), sp(1, "-5.00", 2)];
        let theirs = CurrentState { splits: live(&[sp(0, "-4.00", 1), sp(1, "-6.00", 2)]), category_override: false };
        assert_eq!(decide(&theirs, Some(&ours), &ours), Decision::Skip(SkipReason::UserSplit));
        assert_eq!(undo_decision(&theirs, &ours), UndoDecision::LeaveAlone);
    }

    #[test]
    fn undo_removes_only_a_split_that_is_still_ours() {
        let ours = [sp(0, "-5.00", 1), sp(1, "-5.00", 2)];
        let intact = CurrentState { splits: live(&[ours[1].clone(), ours[0].clone()]), category_override: false };
        assert_eq!(undo_decision(&intact, &ours), UndoDecision::Undo, "row order is irrelevant");
        assert_eq!(undo_decision(&CurrentState::default(), &ours), UndoDecision::LeaveAlone);
    }
}
