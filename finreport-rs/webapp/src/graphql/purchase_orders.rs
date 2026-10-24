//! Amazon purchase orders: the upload mutation and the match-report query.
//!
//! Orders are **evidence about** bank transactions (iteration 6 §2.1), so
//! uploading one only records it: nothing here creates, changes or splits a
//! transaction. Turning an `Apply` match into split parts is a later slice.
//!
//! **How the CSV arrives.** As a `String` argument per file, parsed by
//! `amazon_orders::parse_orders`. This repo has no file-upload plumbing and a
//! personal export is a few hundred KB; the browser reads the file and sends
//! its text. The limits below bound a request.
//!
//! **A bad file is rejected whole, never row by row.** An order is spread
//! over one row per item; importing "the good rows" would store an order
//! with some items missing, which reconciles to nothing and - worse - could
//! reconcile to a *different* charge. The parser likewise fails the file, not
//! the row. Files are independent, though: with several files, the good ones
//! import and each bad one is reported by position with its error.
//!
//! **Privacy.** Error messages are the parser's own: column names and line
//! numbers, never a cell. Nothing in this module logs file content.
//!
//! **Reading.** [`fetch_match_report`] loads the user's orders, their items
//! and the Amazon-looking bank lines in three queries however many orders
//! there are, runs the pure matcher once, and assembles the answer. There is
//! no per-row resolver: every charge a report row prints came back in the
//! candidate query.

use amazon_orders::{
    match_orders, Candidate, Item, MatchKind, MatchReport, MatchStatus, Order, PriceBasis,
    ReviewReason,
};
use async_graphql::{Enum, ErrorExtensions, SimpleObject};
use chrono::Utc;
use entity::entities::{purchase_order, purchase_order_item, transaction};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, TransactionTrait,
};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::graphql::current_user::AuthenticatedUser;
use crate::graphql::events::{kafka_unavailable_error, publish_event};
use crate::graphql::purchase_order_proposals::{self as proposals, ProposalSource};
use crate::graphql::scalars::{Date as GqlDate, Decimal as GqlDecimal, Uuid as GqlUuid};
use crate::kafka::producer::EventPublisher;
use crate::kafka::purchase_orders::{
    PurchaseOrderRecord, CURRENT_SCHEMA_VERSION, SOURCE_AMAZON, TOPIC_PURCHASE_ORDER,
};
use crate::projection::purchase_orders::project_purchase_order;

/// Files per upload, bytes per file and orders per upload. A real export is
/// far below these; they only stop a runaway request.
pub const MAX_FILES: usize = 20;
pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_ORDERS: usize = 5_000;
const ID_CHUNK: usize = 5_000;

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

#[derive(SimpleObject, Clone, Debug, PartialEq)]
pub struct PurchaseOrderItem {
    pub asin: String,
    pub quantity: i32,
    pub title: String,
    /// What this item contributes to the order, as resolved by the parser.
    pub line_total: GqlDecimal,
    /// The export's status text. Free text and localised.
    pub status: String,
    pub returned: bool,
    /// A stored suggestion for this item's category, from a rule or the LLM
    /// cache only (this query never calls a provider). `null` when neither
    /// has an answer, or the cached answer was a decline.
    pub suggested_category_slug: Option<String>,
    /// Where the suggestion came from: `RULE` or `CACHE`, else `NONE`.
    pub suggestion_source: ProposalSource,
}

#[derive(SimpleObject, Clone, Debug, PartialEq)]
pub struct PurchaseOrder {
    pub order_id: String,
    pub order_date: GqlDate,
    pub total: GqlDecimal,
    pub currency: String,
    /// `total` minus the items: shipping, tax, gift cards, promotions.
    pub remainder: GqlDecimal,
    pub items: Vec<PurchaseOrderItem>,
}

/// A bank line the matcher looked at. Light on purpose: a report row, not a
/// full `Transaction`, so it carries no per-row resolvers.
#[derive(SimpleObject, Clone, Debug, PartialEq)]
pub struct PurchaseOrderCharge {
    pub transaction_id: GqlUuid,
    pub booking_date: GqlDate,
    /// Signed, as on the bank statement.
    pub amount: GqlDecimal,
    pub currency: String,
    pub counterparty_name: Option<String>,
    pub description: Option<String>,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PurchaseOrderMatchState {
    /// Exactly one assignment reconciles; `applyPurchaseOrderSplit` accepts it.
    Apply,
    /// A person must look at it; `reasons` say why.
    NeedsReview,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PurchaseOrderMatchKind {
    /// Joined on the order id in the bank description.
    OrderId,
    /// Joined on amount and date only; always needs review.
    AmountAndDate,
}

#[derive(SimpleObject, Clone, Debug, PartialEq)]
pub struct PurchaseOrderReviewReason {
    /// Stable machine name of the reason (`ITEMS_DO_NOT_RECONCILE_TO_CHARGES`, ...).
    pub code: String,
    /// The matcher's own user-facing wording, shown as is.
    pub message: String,
}

#[derive(SimpleObject, Clone, Debug, PartialEq)]
pub struct PurchaseOrderMatch {
    pub order: PurchaseOrder,
    pub kind: PurchaseOrderMatchKind,
    pub state: PurchaseOrderMatchState,
    /// Empty for `APPLY`.
    pub reasons: Vec<PurchaseOrderReviewReason>,
    /// Every debit joined to the order (several for a multi-shipment order).
    pub charges: Vec<PurchaseOrderCharge>,
    /// Credits carrying the order id. Informational; never split.
    pub refunds: Vec<PurchaseOrderCharge>,
    /// Splits already made from this order (`applyPurchaseOrderSplit`).
    pub applications: Vec<crate::graphql::purchase_order_apply::PurchaseOrderApplication>,
}

#[derive(SimpleObject, Clone, Debug, PartialEq, Default)]
pub struct PurchaseOrderMatchReport {
    pub order_count: i32,
    /// Orders joined to at least one charge, whatever their state.
    pub matches: Vec<PurchaseOrderMatch>,
    /// Orders with no bank line: gift-card-only, not yet charged, or outside
    /// the imported bank history.
    pub unmatched_orders: Vec<PurchaseOrder>,
    /// Amazon-looking bank lines no order claims.
    pub unmatched_charges: Vec<PurchaseOrderCharge>,
    /// `D01-` Prime / Music charges, recognised and set aside.
    pub subscription_charges: Vec<PurchaseOrderCharge>,
}

#[derive(SimpleObject, Clone, Debug, PartialEq)]
pub struct PurchaseOrderFileResult {
    /// Position in the `files` argument, from 0.
    pub index: i32,
    pub order_count: i32,
    /// Why the file was rejected: column names and line numbers, never a
    /// cell. `null` when the file was read.
    pub error: Option<String>,
}

#[derive(SimpleObject, Clone, Debug, PartialEq, Default)]
pub struct PurchaseOrderUploadResult {
    pub files: Vec<PurchaseOrderFileResult>,
    /// Orders not seen before.
    pub new_orders: i32,
    /// Orders seen before whose content changed (a status moved, say).
    pub updated_orders: i32,
    /// Orders already stored exactly as uploaded; nothing was published.
    pub unchanged_orders: i32,
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Stable code for a reason; the message is `ReviewReason`'s `Display`.
pub fn reason_code(reason: &ReviewReason) -> &'static str {
    match reason {
        ReviewReason::AmountDateFallback => "AMOUNT_DATE_FALLBACK",
        ReviewReason::AmbiguousFallback { .. } => "AMBIGUOUS_FALLBACK",
        ReviewReason::ChargesDoNotSumToTotal { .. } => "CHARGES_DO_NOT_SUM_TO_TOTAL",
        ReviewReason::ItemsDoNotReconcileToCharges { .. } => "ITEMS_DO_NOT_RECONCILE_TO_CHARGES",
        ReviewReason::AmbiguousItemAssignment { .. } => "AMBIGUOUS_ITEM_ASSIGNMENT",
        ReviewReason::TooManyItems { .. } => "TOO_MANY_ITEMS",
        ReviewReason::OrderItemsDoNotReconcile { .. } => "ORDER_ITEMS_DO_NOT_RECONCILE",
    }
}

fn price_basis_from_str(s: &str) -> PriceBasis {
    match s {
        "unit_price" => PriceBasis::UnitPrice,
        "line_total" => PriceBasis::LineTotal,
        "unreconciled" => PriceBasis::Unreconciled,
        _ => PriceBasis::Indistinguishable,
    }
}

/// Rebuilds the parser's `Order` from its stored rows (the inverse of
/// `projection::purchase_orders`). `items` must be in `position` order.
pub fn order_from_rows(row: &purchase_order::Model, items: &[purchase_order_item::Model]) -> Order {
    Order {
        order_id: row.order_id.clone(),
        order_date: row.order_date,
        total: row.total,
        currency: row.currency.clone(),
        items: items
            .iter()
            .map(|i| Item {
                asin: i.asin.clone(),
                quantity: i.quantity.max(0) as u32,
                price: i.price,
                discount: i.discount,
                promotion: i.promotion.clone(),
                promotion_amount: i.promotion_amount,
                title: i.title.clone(),
                status: i.status.clone(),
                returned: i.returned,
                line_total: i.line_total,
            })
            .collect(),
        price_basis: price_basis_from_str(&row.price_basis),
        discounts_netted: row.discounts_netted,
        remainder: row.remainder,
    }
}

fn to_gql_order(o: &Order) -> PurchaseOrder {
    PurchaseOrder {
        order_id: o.order_id.clone(),
        order_date: GqlDate(o.order_date),
        total: GqlDecimal(o.total),
        currency: o.currency.clone(),
        remainder: GqlDecimal(o.remainder),
        items: o
            .items
            .iter()
            .map(|i| PurchaseOrderItem {
                asin: i.asin.clone(),
                quantity: i.quantity as i32,
                title: i.title.clone(),
                line_total: GqlDecimal(i.line_total),
                status: i.status.clone(),
                returned: i.returned,
                suggested_category_slug: None,
                suggestion_source: ProposalSource::None,
            })
            .collect(),
    }
}

fn to_gql_charge(t: &transaction::Model) -> PurchaseOrderCharge {
    PurchaseOrderCharge {
        transaction_id: GqlUuid(t.id),
        booking_date: GqlDate(t.booking_date),
        amount: GqlDecimal(t.amount),
        currency: t.currency.clone(),
        counterparty_name: t.counterparty_name.clone(),
        description: t.description.clone(),
    }
}

/// What the matcher is shown for a bank line: the counterparty and the
/// remittance text together, so `AMAZON`/`AMZN` and the order id are both
/// visible whichever field carries them.
pub fn candidate_for(t: &transaction::Model) -> Candidate {
    let text = [t.counterparty_name.as_deref(), t.description.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    Candidate {
        id: t.id.to_string(),
        booking_date: t.booking_date,
        amount: t.amount,
        description: text,
    }
}

/// Assembles the GraphQL report from the matcher's output. Pure: `txs` holds
/// the transactions already loaded, keyed by the id the matcher was given.
pub fn assemble_report(
    orders: &[Order],
    report: &MatchReport,
    txs: &HashMap<String, transaction::Model>,
) -> PurchaseOrderMatchReport {
    let by_id: HashMap<&str, &Order> = orders.iter().map(|o| (o.order_id.as_str(), o)).collect();
    let charges_of = |ids: Vec<&str>| -> Vec<PurchaseOrderCharge> {
        ids.into_iter().filter_map(|id| txs.get(id)).map(to_gql_charge).collect()
    };

    let mut matches: Vec<PurchaseOrderMatch> = report
        .matches
        .iter()
        .filter_map(|m| {
            let order = by_id.get(m.order_id.as_str())?;
            let (state, reasons) = match &m.status {
                MatchStatus::Apply => (PurchaseOrderMatchState::Apply, vec![]),
                MatchStatus::NeedsReview(rs) => (
                    PurchaseOrderMatchState::NeedsReview,
                    rs.iter()
                        .map(|r| PurchaseOrderReviewReason {
                            code: reason_code(r).to_string(),
                            message: r.to_string(),
                        })
                        .collect(),
                ),
            };
            Some(PurchaseOrderMatch {
                order: to_gql_order(order),
                kind: match m.kind {
                    MatchKind::OrderId => PurchaseOrderMatchKind::OrderId,
                    MatchKind::AmountAndDate => PurchaseOrderMatchKind::AmountAndDate,
                },
                state,
                reasons,
                charges: charges_of(m.charges.iter().map(|c| c.transaction_id.as_str()).collect()),
                refunds: charges_of(m.refund_transaction_ids.iter().map(String::as_str).collect()),
                applications: vec![],
            })
        })
        .collect();
    matches.sort_by(|a, b| {
        b.order.order_date.cmp(&a.order.order_date).then_with(|| a.order.order_id.cmp(&b.order.order_id))
    });

    let mut unmatched_orders: Vec<PurchaseOrder> = report
        .unmatched_orders
        .iter()
        .filter_map(|id| by_id.get(id.as_str()).map(|o| to_gql_order(o)))
        .collect();
    unmatched_orders.sort_by(|a, b| b.order_date.cmp(&a.order_date).then_with(|| a.order_id.cmp(&b.order_id)));

    let newest_first = |ids: &[String]| {
        let mut v = charges_of(ids.iter().map(String::as_str).collect());
        v.sort_by(|a, b| b.booking_date.cmp(&a.booking_date).then_with(|| a.transaction_id.0.cmp(&b.transaction_id.0)));
        v
    };
    PurchaseOrderMatchReport {
        order_count: orders.len() as i32,
        matches,
        unmatched_orders,
        unmatched_charges: newest_first(&report.unmatched_transactions),
        subscription_charges: newest_first(&report.subscription_transaction_ids),
    }
}

pub(crate) fn validation_error(message: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(message.into()).extend_with(|_, e| e.set("code", "VALIDATION"))
}

/// Parses each file, isolating failures per file, and merges the good ones.
/// Pure, so the limits and the isolation are unit-tested without a database.
pub fn parse_files(files: &[String]) -> Result<(Vec<PurchaseOrderFileResult>, Vec<Order>), String> {
    if files.is_empty() {
        return Err("no files given".into());
    }
    if files.len() > MAX_FILES {
        return Err(format!("at most {MAX_FILES} files per upload"));
    }
    let mut results = Vec::new();
    let mut parsed = Vec::new();
    for (index, content) in files.iter().enumerate() {
        let outcome = if content.len() > MAX_FILE_BYTES {
            Err(format!("file is larger than {} MB", MAX_FILE_BYTES / (1024 * 1024)))
        } else {
            amazon_orders::parse_orders(content.as_bytes()).map_err(|e| e.to_string())
        };
        match outcome {
            Ok(orders) => {
                results.push(PurchaseOrderFileResult { index: index as i32, order_count: orders.len() as i32, error: None });
                parsed.push(orders);
            }
            Err(error) => results.push(PurchaseOrderFileResult { index: index as i32, order_count: 0, error: Some(error) }),
        }
    }
    let merged = amazon_orders::merge_orders(parsed);
    if merged.len() > MAX_ORDERS {
        return Err(format!("at most {MAX_ORDERS} orders per upload"));
    }
    Ok((results, merged))
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

/// The user's stored Amazon orders (all, or just `ids`), rebuilt as parser
/// `Order`s: one query for the orders and one for all their items.
pub(crate) async fn stored_orders(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    ids: Option<&[String]>,
) -> Result<Vec<Order>, sea_orm::DbErr> {
    let orders_q = || {
        purchase_order::Entity::find()
            .filter(purchase_order::Column::UserId.eq(user_id))
            .filter(purchase_order::Column::Source.eq(SOURCE_AMAZON))
    };
    let items_q = || {
        purchase_order_item::Entity::find()
            .filter(purchase_order_item::Column::UserId.eq(user_id))
            .filter(purchase_order_item::Column::Source.eq(SOURCE_AMAZON))
            .order_by_asc(purchase_order_item::Column::OrderId)
            .order_by_asc(purchase_order_item::Column::Position)
    };
    let (mut rows, mut items) = (Vec::new(), Vec::new());
    match ids {
        None => {
            rows = orders_q().all(db).await?;
            items = items_q().all(db).await?;
        }
        Some(ids) => {
            for chunk in ids.chunks(ID_CHUNK) {
                rows.extend(orders_q().filter(purchase_order::Column::OrderId.is_in(chunk.to_vec())).all(db).await?);
                items.extend(items_q().filter(purchase_order_item::Column::OrderId.is_in(chunk.to_vec())).all(db).await?);
            }
        }
    }
    let mut by_order: HashMap<String, Vec<purchase_order_item::Model>> = HashMap::new();
    for item in items {
        by_order.entry(item.order_id.clone()).or_default().push(item);
    }
    Ok(rows
        .iter()
        .map(|r| order_from_rows(r, by_order.get(&r.order_id).map(Vec::as_slice).unwrap_or(&[])))
        .collect())
}

/// `uploadPurchaseOrders`: parse, merge, publish what changed, project it.
/// Re-uploading a file is idempotent: an order already stored exactly as
/// uploaded is counted and skipped, so nothing is republished.
pub async fn upload_purchase_orders(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    settings: &utils::settings::Settings,
    budget: &crate::labeling::budget::SharedLlmBudget,
    user: &AuthenticatedUser,
    files: Vec<String>,
) -> async_graphql::Result<PurchaseOrderUploadResult> {
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let (file_results, orders) = parse_files(&files).map_err(validation_error)?;
    let mut result = PurchaseOrderUploadResult { files: file_results, ..Default::default() };

    let ids: Vec<String> = orders.iter().map(|o| o.order_id.clone()).collect();
    let existing: HashMap<String, Order> = stored_orders(db, user.user_id, Some(&ids))
        .await?
        .into_iter()
        .map(|o| (o.order_id.clone(), o))
        .collect();

    let revision = Utc::now();
    let mut published: Vec<PurchaseOrderRecord> = Vec::new();
    let mut failure: Option<async_graphql::Error> = None;
    for order in orders {
        let is_update = match existing.get(&order.order_id) {
            Some(stored) if *stored == order => {
                result.unchanged_orders += 1;
                continue;
            }
            Some(_) => true,
            None => false,
        };
        let record = PurchaseOrderRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            user_id: user.user_id,
            source: SOURCE_AMAZON.to_string(),
            revision,
            order,
        };
        let Ok(value) = serde_json::to_vec(&record) else {
            failure = Some(async_graphql::Error::new("failed to serialize a purchase order"));
            break;
        };
        if let Err(e) = publish_event(publisher, TOPIC_PURCHASE_ORDER, &record.topic_key(), &value).await {
            failure = Some(e);
            break;
        }
        if is_update {
            result.updated_orders += 1;
        } else {
            result.new_orders += 1;
        }
        published.push(record);
    }

    // Project whatever reached the broker, even when a later publish failed,
    // so the read model never lags our own acknowledged writes.
    let mut warm: Vec<Order> = Vec::new();
    if !published.is_empty() {
        let txn = db.begin().await?;
        for record in published {
            let order_id = record.order.order_id.clone();
            warm.push(record.order.clone());
            project_purchase_order(&txn, user.user_id, SOURCE_AMAZON, &order_id, Some(record)).await?;
        }
        txn.commit().await?;
    }
    if let Some(e) = failure {
        return Err(e);
    }
    // Only the orders just published: a re-upload of unchanged orders stays
    // free. Infallible by design - the suggestions are a convenience.
    proposals::warm_suggestions(db, Some(publisher), settings, budget, &warm).await;
    Ok(result)
}

// ---------------------------------------------------------------------------
// Match report
// ---------------------------------------------------------------------------

/// Everything the matcher saw and said, for a caller that needs more than the
/// assembled report (the apply mutation re-verifies against exactly this).
pub struct MatchInputs {
    pub orders: Vec<Order>,
    pub report: MatchReport,
    /// The Amazon-looking lines of the caller's own accounts, by id string.
    pub transactions: HashMap<String, transaction::Model>,
}

/// Loads the user's orders and candidate bank lines and runs the matcher.
/// Three queries however many orders there are.
pub async fn load_match_inputs(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
) -> async_graphql::Result<MatchInputs> {
    let orders = stored_orders(db, user.user_id, None).await?;
    if orders.is_empty() || user.account_ids.is_empty() {
        // With no orders there is nothing to match against; listing every
        // Amazon line as "unclaimed" would be noise, not information.
        return Ok(MatchInputs { orders, report: MatchReport::default(), transactions: HashMap::new() });
    }
    let amazonish = Condition::any()
        .add(Expr::cust("transaction.description ILIKE '%amazon%'"))
        .add(Expr::cust("transaction.description ILIKE '%amzn%'"))
        .add(Expr::cust("transaction.counterparty_name ILIKE '%amazon%'"))
        .add(Expr::cust("transaction.counterparty_name ILIKE '%amzn%'"));
    let rows = transaction::Entity::find()
        .filter(transaction::Column::AccountId.is_in(user.account_ids.clone()))
        .filter(amazonish)
        .order_by_asc(transaction::Column::BookingDate)
        .order_by_asc(transaction::Column::Id)
        .all(db)
        .await?;
    let candidates: Vec<Candidate> = rows.iter().map(candidate_for).collect();
    let report = match_orders(&orders, &candidates);
    let transactions = rows.into_iter().map(|t| (t.id.to_string(), t)).collect();
    Ok(MatchInputs { orders, report, transactions })
}

/// `purchaseOrderMatches`: the user's stored orders against the Amazon-looking
/// lines of their own accounts, plus the splits already applied from them.
pub async fn fetch_match_report(
    db: &DatabaseConnection,
    settings: &utils::settings::Settings,
    user: &AuthenticatedUser,
) -> async_graphql::Result<PurchaseOrderMatchReport> {
    let inputs = load_match_inputs(db, user).await?;
    if inputs.orders.is_empty() || user.account_ids.is_empty() {
        return Ok(PurchaseOrderMatchReport { order_count: inputs.orders.len() as i32, ..Default::default() });
    }
    let mut out = assemble_report(&inputs.orders, &inputs.report, &inputs.transactions);
    attach_applications(db, user, &mut out).await?;
    // Suggestions are a convenience: a failed lookup leaves the page without
    // them rather than failing the report.
    match proposals::stored_suggestions(db, settings, &inputs.orders).await {
        Ok(suggestions) => attach_suggestions(&mut out, &suggestions),
        Err(e) => tracing::warn!(error = %e, "purchase-order matches: could not load stored suggestions; serving none"),
    }
    Ok(out)
}

/// Fills each item's suggestion from `suggestions` (keyed by order id and
/// item position, as `to_gql_order` lays items out).
fn attach_suggestions(report: &mut PurchaseOrderMatchReport, suggestions: &proposals::StoredSuggestions) {
    let orders = report
        .matches
        .iter_mut()
        .map(|m| &mut m.order)
        .chain(report.unmatched_orders.iter_mut());
    for order in orders {
        for (position, item) in order.items.iter_mut().enumerate() {
            if let Some(d) = suggestions.get(&(order.order_id.clone(), position)) {
                item.suggested_category_slug = d.category_slug.clone();
                item.suggestion_source = d.source;
            }
        }
    }
}

/// Fills each match's `applications` from the application records, flagging
/// those whose live split no longer equals what was applied.
async fn attach_applications(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    report: &mut PurchaseOrderMatchReport,
) -> async_graphql::Result<()> {
    use crate::graphql::purchase_order_apply as apply;
    let rows = apply::applications_for_user(db, user.user_id).await?;
    if rows.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = rows.iter().map(|r| r.transaction_id).collect();
    let live = apply::live_splits(db, &ids).await?;
    let overrides = apply::live_overrides(db, &ids).await?;
    let mut by_order: HashMap<String, Vec<apply::PurchaseOrderApplication>> = HashMap::new();
    for row in &rows {
        let parts: Vec<apply::StoredPart> = serde_json::from_value(row.parts.clone()).unwrap_or_default();
        let state = apply::CurrentState {
            splits: live.get(&row.transaction_id).cloned().unwrap_or_default(),
            category_override: overrides.get(&row.transaction_id).copied(),
        };
        let intact = apply::application_intact(&parts, &state);
        by_order.entry(row.order_id.clone()).or_default().push(apply::PurchaseOrderApplication {
            transaction_id: GqlUuid(row.transaction_id),
            applied_at: crate::graphql::scalars::DateTime(row.applied_at.with_timezone(&Utc)),
            parts: apply::to_applied_parts(&parts),
            applied_as: apply::applied_as(&parts),
            intact,
        });
    }
    for m in &mut report.matches {
        if let Some(mut apps) = by_order.remove(&m.order.order_id) {
            apps.sort_by_key(|a| a.transaction_id.0);
            m.applications = apps;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, Utc};
    use rust_decimal::Decimal;
    use sea_orm::prelude::DateTimeWithTimeZone;

    const HEADER: &str = "Order ID,Order Date,Total Amount,Total Savings,Status,Item ASIN,Item Quantity,Item Price,Item Discount,Promotions,Item Title,Item URL,Details URL,Recipient Name,Recipient Street,Recipient City,Recipient State,Recipient Zip,Recipient Country";

    fn good_csv() -> String {
        format!(
            "{HEADER}\n304-9357930-8158714,2026-09-23,22.59 EUR,,Delivered,B000TEST01,1,22.59,,,Kaesemesser,u,d,Secret Name,Secret Street 1,Secretville,,00000,DE\n"
        )
    }

    #[test]
    fn a_bad_file_is_rejected_whole_and_does_not_block_a_good_one() {
        let bad = format!("{HEADER}\n304-9357930-8158714,not-a-date,22.59 EUR,,Delivered,B0,1,22.59,,,T,u,d,Secret Name,Secret Street 1,Secretville,,00000,DE\n");
        let (results, orders) = parse_files(&[bad, good_csv()]).unwrap();
        assert_eq!(orders.len(), 1);
        assert!(results[0].error.is_some());
        assert_eq!(results[1].error, None);
        assert_eq!((results[0].index, results[1].index), (0, 1));
    }

    #[test]
    fn errors_name_columns_and_lines_but_never_echo_a_cell() {
        let bad = format!("{HEADER}\n304-9357930-8158714,not-a-date,22.59 EUR,,Delivered,B0,1,22.59,,,T,u,d,Secret Name,Secret Street 1,Secretville,,00000,DE\n");
        let truncated = "Order ID,Order Date\nSecret Name,Secret Street 1\n".to_string();
        let wrong_header = "Foo,Bar\nSecret Name,Secret Street 1\n".to_string();
        let (results, _) = parse_files(&[bad, truncated, wrong_header]).unwrap();
        for r in &results {
            let msg = r.error.as_deref().expect("every file is bad");
            for leak in ["Secret", "Secretville", "00000", "not-a-date"] {
                assert!(!msg.contains(leak), "error echoed a cell: {msg}");
            }
        }
    }

    #[test]
    fn same_order_in_two_files_is_one_order() {
        let (_, orders) = parse_files(&[good_csv(), good_csv()]).unwrap();
        assert_eq!(orders.len(), 1);
    }

    #[test]
    fn limits_are_enforced() {
        assert!(parse_files(&[]).is_err());
        assert!(parse_files(&vec![good_csv(); MAX_FILES + 1]).is_err());
        let huge = "x".repeat(MAX_FILE_BYTES + 1);
        let (results, orders) = parse_files(&[huge]).unwrap();
        assert!(orders.is_empty());
        assert!(results[0].error.as_deref().unwrap().contains("larger"));
    }

    #[test]
    fn every_reason_has_a_distinct_code() {
        let d = Decimal::ONE;
        let reasons = [
            ReviewReason::AmountDateFallback,
            ReviewReason::AmbiguousFallback { candidates: 2 },
            ReviewReason::ChargesDoNotSumToTotal { charged: d, total: d },
            ReviewReason::ItemsDoNotReconcileToCharges { charges: 2 },
            ReviewReason::AmbiguousItemAssignment { charges: 2 },
            ReviewReason::TooManyItems { items: 13 },
            ReviewReason::OrderItemsDoNotReconcile { remainder: d },
        ];
        let codes: std::collections::HashSet<_> = reasons.iter().map(reason_code).collect();
        assert_eq!(codes.len(), reasons.len());
    }

    fn tx(id: u128, date: (i32, u32, u32), amount: &str, name: &str, description: &str) -> transaction::Model {
        let now: DateTimeWithTimeZone = Utc::now().into();
        transaction::Model {
            id: Uuid::from_u128(id),
            account_id: Uuid::from_u128(99),
            source: "test".into(),
            external_id: id.to_string(),
            booking_date: NaiveDate::from_ymd_opt(date.0, date.1, date.2).unwrap(),
            valuta_date: None,
            booking_status: "BOOKED".into(),
            amount: amount.parse().unwrap(),
            currency: "EUR".into(),
            counterparty_name: Some(name.into()),
            counterparty_iban: None,
            description: Some(description.into()),
            transaction_type: None,
            raw_payload: serde_json::json!({}),
            origin: "test".into(),
            imported_at: now,
            updated_at: now,
            counterparty_key: None,
        }
    }

    fn orders() -> Vec<Order> {
        let csv = format!(
            "{HEADER}\n\
             304-9357930-8158714,2026-09-23,22.59 EUR,,Delivered,B1,1,13.60,,,Big,u,d,a,b,c,,0,DE\n\
             304-9357930-8158714,2026-09-23,22.59 EUR,,Delivered,B2,1,8.99,,,Small,u,d,a,b,c,,0,DE\n\
             303-9169011-8123555,2026-10-08,13.45 EUR,,Delivered,B3,1,13.45,,,Single,u,d,a,b,c,,0,DE\n\
             111-1111111-1111111,2026-10-01,5.00 EUR,,Delivered,B4,1,5.00,,,Gift,u,d,a,b,c,,0,DE\n"
        );
        amazon_orders::parse_orders(csv.as_bytes()).unwrap()
    }

    #[test]
    fn report_separates_apply_review_subscription_and_the_unmatched() {
        let orders = orders();
        let amazon = "AMAZON PAYMENTS EUROPE S.C.A.";
        let rows = vec![
            tx(1, (2026, 10, 8), "-13.45", amazon, "01303-9169011-8123555 AMZN Mktp DE"),
            tx(2, (2026, 9, 24), "-13.60", amazon, "01304-9357930-8158714 AMZN Mktp DE"),
            tx(3, (2026, 9, 23), "-8.99", amazon, "01304-9357930-8158714 AMZN Mktp DE"),
            tx(4, (2026, 10, 7), "-8.99", "AMAZON EU S.A R.L.", "01D01-8109513-9170236 AMZNPrime DE"),
            tx(5, (2026, 10, 2), "-7.77", amazon, "01999-9999999-9999999 AMZN Mktp DE"),
        ];
        let candidates: Vec<Candidate> = rows.iter().map(candidate_for).collect();
        let report = match_orders(&orders, &candidates);
        let txs = rows.into_iter().map(|t| (t.id.to_string(), t)).collect();
        let out = assemble_report(&orders, &report, &txs);

        assert_eq!(out.order_count, 3);
        let by_id = |id: &str| out.matches.iter().find(|m| m.order.order_id == id).unwrap();
        let single = by_id("303-9169011-8123555");
        assert_eq!(single.state, PurchaseOrderMatchState::Apply);
        assert!(single.reasons.is_empty());
        assert_eq!(single.charges.len(), 1);

        // 13.60 + 8.99 matches the items one-to-one, so this one reconciles.
        assert_eq!(by_id("304-9357930-8158714").charges.len(), 2);

        assert_eq!(out.subscription_charges.len(), 1);
        assert_eq!(out.unmatched_orders.iter().map(|o| o.order_id.as_str()).collect::<Vec<_>>(), ["111-1111111-1111111"]);
        assert_eq!(out.unmatched_charges.len(), 1, "the unknown order id is unclaimed");
        assert!(out.matches.iter().all(|m| m.order.order_id != "111-1111111-1111111"));
    }

    #[test]
    fn a_multi_charge_order_that_cannot_be_assigned_carries_the_matchers_wording() {
        let csv = format!(
            "{HEADER}\n\
             304-9357930-8158714,2026-09-23,22.59 EUR,,Delivered,B1,1,22.59,,,Only,u,d,a,b,c,,0,DE\n"
        );
        let orders = amazon_orders::parse_orders(csv.as_bytes()).unwrap();
        let amazon = "AMAZON PAYMENTS EUROPE S.C.A.";
        let rows = vec![
            tx(2, (2026, 9, 24), "-13.60", amazon, "01304-9357930-8158714 AMZN Mktp DE"),
            tx(3, (2026, 9, 23), "-8.99", amazon, "01304-9357930-8158714 AMZN Mktp DE"),
        ];
        let candidates: Vec<Candidate> = rows.iter().map(candidate_for).collect();
        let report = match_orders(&orders, &candidates);
        let txs = rows.into_iter().map(|t| (t.id.to_string(), t)).collect();
        let out = assemble_report(&orders, &report, &txs);
        let m = &out.matches[0];
        assert_eq!(m.state, PurchaseOrderMatchState::NeedsReview);
        assert_eq!(m.reasons[0].code, "ITEMS_DO_NOT_RECONCILE_TO_CHARGES");
        assert_eq!(m.reasons[0].message, "order spans 2 charges; items do not reconcile to them");
    }

    #[test]
    fn rows_round_trip_to_the_parsers_order() {
        let order = orders().remove(0);
        let row = purchase_order::Model {
            user_id: Uuid::nil(),
            source: SOURCE_AMAZON.into(),
            order_id: order.order_id.clone(),
            order_date: order.order_date,
            total: order.total,
            currency: order.currency.clone(),
            price_basis: "indistinguishable".into(),
            discounts_netted: order.discounts_netted,
            remainder: order.remainder,
            revision: Utc::now().into(),
        };
        let items: Vec<purchase_order_item::Model> = order
            .items
            .iter()
            .enumerate()
            .map(|(p, i)| purchase_order_item::Model {
                user_id: Uuid::nil(),
                source: SOURCE_AMAZON.into(),
                order_id: order.order_id.clone(),
                position: p as i32,
                asin: i.asin.clone(),
                quantity: i.quantity as i32,
                price: i.price,
                discount: i.discount,
                promotion: i.promotion.clone(),
                promotion_amount: i.promotion_amount,
                title: i.title.clone(),
                status: i.status.clone(),
                returned: i.returned,
                line_total: i.line_total,
            })
            .collect();
        assert_eq!(order_from_rows(&row, &items), order);
    }
}
