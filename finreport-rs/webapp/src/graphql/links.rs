//! Transaction links: the user's declaration that some transactions offset
//! others - today a reimbursement repaying an expense. See
//! `crate::links` for the arithmetic and `crate::kafka::links` for the
//! record; this module is the GraphQL face of both.
//!
//! What it deliberately does **not** do is rewrite history. Linking a
//! September bill to an October reimbursement changes no period total: both
//! transactions keep showing the money that actually moved. The link is
//! exposed on every `Transaction` (`Transaction.link`) so a view can
//! *annotate* it, `reimbursementSummary` gives the netted figure as a
//! separate number, and `TransactionFilter.reimbursements` lets an income
//! view leave reimbursements out. Removing a link (a tombstone) restores the
//! unlinked view exactly, because nothing but the link was ever written.
//!
//! **Reading.** The figures are derived at read time from the members' own
//! amounts. `Transaction.link` is a `#[ComplexObject]` resolver, so a list
//! would cost a query per row; the list resolvers (`transactions`,
//! `reviewQueue`, `goalTransactions`) therefore [`prefetch`] a whole page
//! into a per-request [`LinkCache`] in a fixed four queries. Anywhere that
//! does not prefetch still works, at one cheap indexed lookup per row (and
//! the rest only for rows that are actually linked).
//!
//! **Writing** follows the other human-decision topics: publish the
//! whole-state record, then apply the same projection in the caller's own
//! request so the answer reads back at once. A transaction is in at most one
//! link (an expense with several reimbursements is one link with several
//! offsets), enforced here at write time; the projection itself stays
//! permissive so a replay can never fail on it.

use async_graphql::{Enum, ErrorExtensions, InputObject, SimpleObject};
use chrono::Utc;
use entity::entities::{transaction, transaction_link, transaction_link_member};
use rust_decimal::Decimal;
use sea_orm::sea_query::{Expr, Query};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, Order, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::graphql::current_user::{current_user, scoped_account_ids, AuthenticatedUser};
use crate::graphql::events::{kafka_unavailable_error, publish_event, publish_tombstone};
use crate::graphql::scalars::{
    Date as GqlDate, DateTime as GqlDateTime, Decimal as GqlDecimal, Uuid as GqlUuid,
};
use crate::graphql::transactions::{build_condition_with_category_filters, to_graphql_transaction};
use crate::graphql::types::{Direction, Transaction, TransactionFilter};
use crate::kafka::links::{
    LinkKind, LinkMemberRef, LinkRole, TransactionLinkRecord, CURRENT_SCHEMA_VERSION,
    TOPIC_TRANSACTION_LINK,
};
use crate::kafka::producer::EventPublisher;
use crate::links::{self, LinkStatus, Slot};
use crate::projection::links::{project_transaction_link_with_ids, project_transaction_link};

/// A link has at most this many members in total; a hand-made link is a
/// handful, and an unbounded one is a typo or an abuse.
pub const MAX_LINK_MEMBERS: usize = 50;
const MAX_NOTE_CHARS: usize = 500;
/// `IN (...)` lists are chunked below Postgres' bind-parameter ceiling.
const ID_CHUNK: usize = 5_000;

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum TransactionLinkKind {
    /// Money paid out, later paid back (wholly or in part) by someone else.
    Reimbursement,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum TransactionLinkRole {
    /// The side being offset: the bill that was paid.
    Expense,
    /// The side doing the offsetting: the money that came back.
    Offset,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum TransactionLinkStatus {
    /// The offsetting side exactly covers the expense side.
    Full,
    /// Something came back, but less than was paid.
    Partial,
    /// More came back than was paid; the excess is `surplus`.
    Over,
    /// A side has no member left to count (it was removed, or is not
    /// visible to the caller): nothing is offset.
    Incomplete,
}

/// One transaction in a link, with its share of the offset.
#[derive(SimpleObject, Clone, Debug)]
pub struct TransactionLinkMember {
    pub transaction_id: GqlUuid,
    pub role: TransactionLinkRole,
    /// `null` when the transaction no longer exists or is not the caller's.
    pub booking_date: Option<GqlDate>,
    /// Signed, as on the transaction itself.
    pub amount: Option<GqlDecimal>,
    pub currency: Option<String>,
    pub counterparty_name: Option<String>,
    pub description: Option<String>,
    /// For an expense: how much of it is offset. For an offset: how much of
    /// it goes to offsetting. A magnitude, never negative.
    pub allocated: GqlDecimal,
    /// For an expense: what it still costs. For an offset: the part that
    /// offsets nothing (over-reimbursement). A magnitude.
    pub remaining: GqlDecimal,
}

/// A user-declared link and the figures derived from its members. Nothing
/// here is stored: all of it follows the members' current amounts.
#[derive(SimpleObject, Clone, Debug)]
pub struct TransactionLink {
    pub id: GqlUuid,
    pub kind: TransactionLinkKind,
    pub note: Option<String>,
    pub status: TransactionLinkStatus,
    pub currency: String,
    /// Sum of the expense members' magnitudes.
    pub expense_total: GqlDecimal,
    /// Sum of the offsetting members' magnitudes.
    pub offset_total: GqlDecimal,
    /// What actually offsets: the smaller of the two totals.
    pub reimbursed: GqlDecimal,
    /// What the expense side still costs after the offset: never negative.
    pub net: GqlDecimal,
    /// How much more came back than was paid; zero unless `status` is `OVER`.
    pub surplus: GqlDecimal,
    /// Members that could not be found, left out of every figure above.
    pub missing_members: i32,
    pub members: Vec<TransactionLinkMember>,
    pub updated_at: GqlDateTime,
}

#[derive(InputObject, Debug)]
pub struct TransactionLinkInput {
    /// Defaults to `REIMBURSEMENT`.
    pub kind: Option<TransactionLinkKind>,
    /// The side being offset. At least one.
    pub expense_ids: Vec<GqlUuid>,
    /// The side doing the offsetting. At least one.
    pub offset_ids: Vec<GqlUuid>,
    pub note: Option<String>,
}

/// A transaction that could be linked to another, with how well it fits.
#[derive(SimpleObject)]
pub struct LinkCandidate {
    pub transaction: Transaction,
    /// 0..1: closer in amount and nearer in date scores higher.
    pub score: f64,
}

/// The netted figure for the transactions a filter matches, kept apart from
/// the period totals on purpose: those keep showing the money that actually
/// left the account.
#[derive(SimpleObject, Debug, PartialEq)]
pub struct ReimbursementSummary {
    /// Sum of the magnitudes of the matched expense-side transactions.
    pub expense_total: GqlDecimal,
    /// How much of that is offset by linked reimbursements, wherever and
    /// whenever those landed.
    pub reimbursed: GqlDecimal,
    /// `expenseTotal - reimbursed`.
    pub net: GqlDecimal,
    /// How many matched transactions are on a link's expense side.
    pub linked_count: i32,
    /// How many of those are only partly offset.
    pub partially_reimbursed_count: i32,
}

fn kind_to_gql(kind: LinkKind) -> TransactionLinkKind {
    match kind {
        LinkKind::Reimbursement => TransactionLinkKind::Reimbursement,
    }
}

fn kind_from_gql(kind: TransactionLinkKind) -> LinkKind {
    match kind {
        TransactionLinkKind::Reimbursement => LinkKind::Reimbursement,
    }
}

fn role_to_gql(role: LinkRole) -> TransactionLinkRole {
    match role {
        LinkRole::Expense => TransactionLinkRole::Expense,
        LinkRole::Offset => TransactionLinkRole::Offset,
    }
}

fn status_to_gql(status: LinkStatus) -> TransactionLinkStatus {
    match status {
        LinkStatus::Full => TransactionLinkStatus::Full,
        LinkStatus::Partial => TransactionLinkStatus::Partial,
        LinkStatus::Over => TransactionLinkStatus::Over,
        LinkStatus::Incomplete => TransactionLinkStatus::Incomplete,
    }
}

fn validation_error(message: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(message.into()).extend_with(|_, e| e.set("code", "VALIDATION"))
}

fn not_found_error() -> async_graphql::Error {
    async_graphql::Error::new("transaction or link not found or not accessible")
}

// ---------------------------------------------------------------------------
// Building the view of a link
// ---------------------------------------------------------------------------

/// The view of one link: its stored declaration, its member rows, and the
/// transactions (already restricted to what the caller may see) they point
/// at. Pure, so the missing-member behaviour is unit-tested directly.
pub(crate) fn build_link(
    link: &transaction_link::Model,
    members: &[transaction_link_member::Model],
    transactions: &HashMap<Uuid, transaction::Model>,
) -> TransactionLink {
    // A role the row cannot be parsed into would be a corrupt projection;
    // treat the member as an expense rather than dropping it silently.
    let mut ordered: Vec<&transaction_link_member::Model> = members.iter().collect();
    ordered.sort_by_key(|m| (m.role.clone(), m.transaction_id));
    let slots: Vec<Slot> = ordered
        .iter()
        .map(|m| Slot {
            transaction_id: m.transaction_id,
            role: LinkRole::parse(&m.role).unwrap_or(LinkRole::Expense),
            amount: transactions.get(&m.transaction_id).map(|t| t.amount),
        })
        .collect();
    let offsets = links::compute(&slots);

    let currency = ordered
        .iter()
        .find_map(|m| transactions.get(&m.transaction_id))
        .map(|t| t.currency.clone())
        .unwrap_or_else(|| "EUR".to_string());

    let members = offsets
        .shares
        .iter()
        .map(|share| {
            let tx = transactions.get(&share.transaction_id);
            TransactionLinkMember {
                transaction_id: GqlUuid(share.transaction_id),
                role: role_to_gql(share.role),
                booking_date: tx.map(|t| GqlDate(t.booking_date)),
                amount: tx.map(|t| GqlDecimal(t.amount)),
                currency: tx.map(|t| t.currency.clone()),
                counterparty_name: tx.and_then(|t| t.counterparty_name.clone()),
                description: tx.and_then(|t| t.description.clone()),
                allocated: GqlDecimal(share.allocated),
                remaining: GqlDecimal(share.remaining),
            }
        })
        .collect();

    TransactionLink {
        id: GqlUuid(link.id),
        kind: LinkKind::parse(&link.kind).map(kind_to_gql).unwrap_or(TransactionLinkKind::Reimbursement),
        note: link.note.clone(),
        status: status_to_gql(offsets.status),
        currency,
        expense_total: GqlDecimal(offsets.expense_total),
        offset_total: GqlDecimal(offsets.offset_total),
        reimbursed: GqlDecimal(offsets.reimbursed),
        net: GqlDecimal(offsets.net),
        surplus: GqlDecimal(offsets.surplus),
        missing_members: offsets.missing as i32,
        members,
        updated_at: GqlDateTime(link.revision.with_timezone(&Utc)),
    }
}

/// The views of the links `link_ids`, in two queries after the link rows
/// (members, then the member transactions the caller may see).
async fn links_by_ids(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    link_ids: &[Uuid],
) -> async_graphql::Result<HashMap<Uuid, TransactionLink>> {
    if link_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut links = Vec::new();
    let mut members = Vec::new();
    for chunk in link_ids.chunks(ID_CHUNK) {
        links.extend(
            transaction_link::Entity::find()
                .filter(transaction_link::Column::Id.is_in(chunk.to_vec()))
                .all(db)
                .await?,
        );
        members.extend(
            transaction_link_member::Entity::find()
                .filter(transaction_link_member::Column::LinkId.is_in(chunk.to_vec()))
                .all(db)
                .await?,
        );
    }
    let member_tx_ids: Vec<Uuid> = {
        let mut ids: Vec<Uuid> = members.iter().map(|m| m.transaction_id).collect();
        ids.sort();
        ids.dedup();
        ids
    };
    let mut transactions: HashMap<Uuid, transaction::Model> = HashMap::new();
    for chunk in member_tx_ids.chunks(ID_CHUNK) {
        for tx in transaction::Entity::find()
            .filter(transaction::Column::Id.is_in(chunk.to_vec()))
            .filter(transaction::Column::AccountId.is_in(scoped_ids.to_vec()))
            .all(db)
            .await?
        {
            transactions.insert(tx.id, tx);
        }
    }
    let mut by_link: HashMap<Uuid, Vec<transaction_link_member::Model>> = HashMap::new();
    for m in members {
        by_link.entry(m.link_id).or_default().push(m);
    }
    Ok(links
        .iter()
        .map(|l| {
            let rows = by_link.remove(&l.id).unwrap_or_default();
            (l.id, build_link(l, &rows, &transactions))
        })
        .collect())
}

/// Which link each of `transaction_ids` is on, as the link's view. A
/// transaction on more than one link (which `createTransactionLink` refuses
/// but a hand-fed topic could produce) gets the most recently revised, ties
/// broken by id, so the answer is stable.
pub(crate) async fn links_for_transactions(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    transaction_ids: &[Uuid],
) -> async_graphql::Result<HashMap<Uuid, TransactionLink>> {
    if transaction_ids.is_empty() || scoped_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut member_rows = Vec::new();
    for chunk in transaction_ids.chunks(ID_CHUNK) {
        member_rows.extend(
            transaction_link_member::Entity::find()
                .filter(transaction_link_member::Column::TransactionId.is_in(chunk.to_vec()))
                .all(db)
                .await?,
        );
    }
    if member_rows.is_empty() {
        return Ok(HashMap::new());
    }
    let link_ids: Vec<Uuid> = {
        let mut ids: Vec<Uuid> = member_rows.iter().map(|m| m.link_id).collect();
        ids.sort();
        ids.dedup();
        ids
    };
    let revisions: HashMap<Uuid, chrono::DateTime<chrono::FixedOffset>> = transaction_link::Entity::find()
        .filter(transaction_link::Column::Id.is_in(link_ids.clone()))
        .all(db)
        .await?
        .into_iter()
        .map(|l| (l.id, l.revision))
        .collect();

    let mut chosen: HashMap<Uuid, Uuid> = HashMap::new();
    for m in &member_rows {
        let Some(rev) = revisions.get(&m.link_id) else { continue };
        let better = match chosen.get(&m.transaction_id) {
            None => true,
            Some(current) => (rev, m.link_id) > (&revisions[current], *current),
        };
        if better {
            chosen.insert(m.transaction_id, m.link_id);
        }
    }
    let wanted: Vec<Uuid> = {
        let mut ids: Vec<Uuid> = chosen.values().copied().collect();
        ids.sort();
        ids.dedup();
        ids
    };
    let views = links_by_ids(db, scoped_ids, &wanted).await?;
    Ok(chosen
        .into_iter()
        .filter_map(|(tx, link)| views.get(&link).cloned().map(|v| (tx, v)))
        .collect())
}

// ---------------------------------------------------------------------------
// Per-request prefetch (N+1 avoidance)
// ---------------------------------------------------------------------------

/// Per-request cache for `Transaction.link`, filled for a whole page by
/// [`prefetch`] so a list does not issue queries per row. Inserted fresh per
/// request by `request_with_auth`; a resolver that finds nothing here falls
/// back to its own lookup, so correctness never depends on it being warm.
#[derive(Default)]
pub struct LinkCache {
    links: RwLock<HashMap<Uuid, Option<TransactionLink>>>,
}

/// Loads the links of `transaction_ids` in a fixed number of queries and
/// stores them (and, as `None`, the absence of one) in `cache`.
pub async fn prefetch(
    db: &DatabaseConnection,
    cache: &LinkCache,
    scoped_ids: &[Uuid],
    transaction_ids: &[Uuid],
) -> async_graphql::Result<()> {
    if transaction_ids.is_empty() {
        return Ok(());
    }
    let mut found = links_for_transactions(db, scoped_ids, transaction_ids).await?;
    let mut links = cache.links.write().await;
    for id in transaction_ids {
        links.insert(*id, found.remove(id));
    }
    Ok(())
}

/// Primes the request's [`LinkCache`] for a page of already-fetched
/// transactions; a no-op when the request carries no cache.
pub async fn prefetch_page<'a>(
    ctx: &async_graphql::Context<'_>,
    transactions: impl IntoIterator<Item = &'a Transaction>,
) -> async_graphql::Result<()> {
    let Ok(cache) = ctx.data::<LinkCache>() else { return Ok(()) };
    let user = current_user(ctx)?;
    let db: &Arc<DatabaseConnection> = ctx.data()?;
    let ids: Vec<Uuid> = transactions.into_iter().map(|t| t.id.0).collect();
    prefetch(db, cache, &user.account_ids, &ids).await
}

/// `Transaction.link`: the cache first, a lookup of its own otherwise.
pub async fn link_for(
    ctx: &async_graphql::Context<'_>,
    transaction_id: Uuid,
) -> async_graphql::Result<Option<TransactionLink>> {
    if let Ok(cache) = ctx.data::<LinkCache>()
        && let Some(found) = cache.links.read().await.get(&transaction_id)
    {
        return Ok(found.clone());
    }
    let user = current_user(ctx)?;
    let db: &Arc<DatabaseConnection> = ctx.data()?;
    Ok(links_for_transactions(db, &user.account_ids, &[transaction_id])
        .await?
        .remove(&transaction_id))
}

// ---------------------------------------------------------------------------
// Validation (pure)
// ---------------------------------------------------------------------------

fn normalize_note(note: Option<String>) -> async_graphql::Result<Option<String>> {
    let Some(note) = note else { return Ok(None) };
    let note = note.trim().to_string();
    if note.is_empty() {
        return Ok(None);
    }
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err(validation_error(format!("note must be at most {MAX_NOTE_CHARS} characters")));
    }
    Ok(Some(note))
}

fn dedupe(ids: &[GqlUuid]) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    ids.iter().map(|i| i.0).filter(|i| seen.insert(*i)).collect()
}

/// The rules a set of members must satisfy for a link of `kind`, given the
/// transactions themselves. Pure.
fn check_members(
    kind: LinkKind,
    expense: &[transaction::Model],
    offset: &[transaction::Model],
) -> Result<(), String> {
    if expense.is_empty() || offset.is_empty() {
        return Err("a link needs at least one expense and one offsetting transaction".into());
    }
    if expense.len() + offset.len() > MAX_LINK_MEMBERS {
        return Err(format!("a link can hold at most {MAX_LINK_MEMBERS} transactions"));
    }
    let expense_ids: HashSet<Uuid> = expense.iter().map(|t| t.id).collect();
    if offset.iter().any(|t| expense_ids.contains(&t.id)) {
        return Err("a transaction cannot be on both sides of a link".into());
    }
    for (role, side) in [(LinkRole::Expense, expense), (LinkRole::Offset, offset)] {
        if let Some(bad) = side.iter().find(|t| !links::amount_fits_role(kind, role, t.amount)) {
            let want = match role {
                LinkRole::Expense => "money paid out (a negative amount)",
                LinkRole::Offset => "money received (a positive amount)",
            };
            return Err(format!(
                "{} on the {} side must be {want}",
                bad.counterparty_name.as_deref().unwrap_or("a transaction"),
                role.as_str()
            ));
        }
    }
    let currencies: HashSet<&str> = expense.iter().chain(offset).map(|t| t.currency.as_str()).collect();
    if currencies.len() > 1 {
        return Err("all transactions in a link must share one currency".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// Loads `ids` from the caller's own accounts; any id that is missing or
/// someone else's fails the whole call, indistinguishably.
async fn load_scoped(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    ids: &[Uuid],
) -> async_graphql::Result<HashMap<Uuid, transaction::Model>> {
    let rows = transaction::Entity::find()
        .filter(transaction::Column::Id.is_in(ids.to_vec()))
        .filter(transaction::Column::AccountId.is_in(scoped_ids.to_vec()))
        .all(db)
        .await?;
    let map: HashMap<Uuid, transaction::Model> = rows.into_iter().map(|r| (r.id, r)).collect();
    if ids.iter().any(|id| !map.contains_key(id)) {
        return Err(not_found_error());
    }
    Ok(map)
}

/// The stored link `link_id` if the caller may manage it: they declared it,
/// or every member that still exists is in one of their accounts (and at
/// least one does). Anything else is "not found", so a link's existence
/// does not leak.
async fn manageable_link(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    link_id: Uuid,
) -> async_graphql::Result<transaction_link::Model> {
    let link = transaction_link::Entity::find_by_id(link_id)
        .one(db)
        .await?
        .ok_or_else(not_found_error)?;
    if link.owner_user_id == Some(user.user_id) {
        return Ok(link);
    }
    let member_ids: Vec<Uuid> = transaction_link_member::Entity::find()
        .filter(transaction_link_member::Column::LinkId.eq(link_id))
        .all(db)
        .await?
        .into_iter()
        .map(|m| m.transaction_id)
        .collect();
    let existing = transaction::Entity::find()
        .filter(transaction::Column::Id.is_in(member_ids))
        .all(db)
        .await?;
    if existing.is_empty() || existing.iter().any(|t| !user.account_ids.contains(&t.account_id)) {
        return Err(not_found_error());
    }
    Ok(link)
}

/// Creates (`existing = None`) or replaces (`Some(id)`) a link.
async fn save_link(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    existing: Option<Uuid>,
    input: TransactionLinkInput,
) -> async_graphql::Result<TransactionLink> {
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    let scoped_ids = scoped_account_ids(user, None)?;
    let kind = kind_from_gql(input.kind.unwrap_or(TransactionLinkKind::Reimbursement));
    let note = normalize_note(input.note)?;

    let expense_ids = dedupe(&input.expense_ids);
    let offset_ids = dedupe(&input.offset_ids);
    if expense_ids.len() + offset_ids.len() > MAX_LINK_MEMBERS {
        return Err(validation_error(format!("a link can hold at most {MAX_LINK_MEMBERS} transactions")));
    }
    let all_ids: Vec<Uuid> = expense_ids.iter().chain(&offset_ids).copied().collect();
    let found = load_scoped(db, &scoped_ids, &all_ids).await?;
    let pick = |ids: &[Uuid]| -> Vec<transaction::Model> { ids.iter().map(|i| found[i].clone()).collect() };
    let (expense, offset) = (pick(&expense_ids), pick(&offset_ids));
    check_members(kind, &expense, &offset).map_err(validation_error)?;

    let link_id = match existing {
        Some(id) => {
            manageable_link(db, user, id).await?;
            id
        }
        None => Uuid::new_v4(),
    };

    // One transaction, one link: refuse anything already on another.
    let taken = transaction_link_member::Entity::find()
        .filter(transaction_link_member::Column::TransactionId.is_in(all_ids.clone()))
        .filter(transaction_link_member::Column::LinkId.ne(link_id))
        .all(db)
        .await?;
    if !taken.is_empty() {
        return Err(async_graphql::Error::new(
            "a transaction is already part of another link; edit that link instead",
        )
        .extend_with(|_, e| e.set("code", "ALREADY_LINKED")));
    }

    let owner = match existing {
        Some(id) => transaction_link::Entity::find_by_id(id)
            .one(db)
            .await?
            .and_then(|l| l.owner_user_id)
            .or(Some(user.user_id)),
        None => Some(user.user_id),
    };
    let member_ref = |t: &transaction::Model, role: LinkRole| LinkMemberRef {
        source: t.source.clone(),
        external_id: t.external_id.clone(),
        role,
    };
    let record = TransactionLinkRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        id: link_id,
        kind,
        owner_user_id: owner,
        members: expense
            .iter()
            .map(|t| member_ref(t, LinkRole::Expense))
            .chain(offset.iter().map(|t| member_ref(t, LinkRole::Offset)))
            .collect(),
        note,
        revision: Utc::now(),
    };
    let value = serde_json::to_vec(&record)
        .map_err(|e| async_graphql::Error::new(format!("failed to serialize transaction link: {e}")))?;
    publish_event(publisher, TOPIC_TRANSACTION_LINK, &link_id.to_string(), &value).await?;
    let txn = db.begin().await?;
    let member_ids: Vec<Uuid> = expense.iter().chain(&offset).map(|t| t.id).collect();
    project_transaction_link_with_ids(&txn, link_id, Some(record), &member_ids).await?;
    txn.commit().await?;

    links_by_ids(db, &scoped_ids, &[link_id])
        .await?
        .remove(&link_id)
        .ok_or_else(|| async_graphql::Error::new("transaction link did not take effect"))
}

pub async fn create_transaction_link(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    input: TransactionLinkInput,
) -> async_graphql::Result<TransactionLink> {
    save_link(db, publisher, user, None, input).await
}

/// Replaces a link's members and note wholesale: the way to add a second
/// reimbursement to an expense, or drop one that was linked by mistake.
pub async fn update_transaction_link(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    id: Uuid,
    input: TransactionLinkInput,
) -> async_graphql::Result<TransactionLink> {
    save_link(db, publisher, user, Some(id), input).await
}

/// `removeTransactionLink`: a tombstone, then the matching delete. Returns
/// whether a link existed. Nothing else was ever written on the linked
/// transactions, so this restores the previous view exactly.
pub async fn remove_transaction_link(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    user: &AuthenticatedUser,
    id: Uuid,
) -> async_graphql::Result<bool> {
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    if manageable_link(db, user, id).await.is_err() {
        return Ok(false);
    }
    publish_tombstone(publisher, TOPIC_TRANSACTION_LINK, &id.to_string()).await?;
    let txn = db.begin().await?;
    project_transaction_link(&txn, id, None).await?;
    txn.commit().await?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// `transactionLink(id)`: visible when the caller declared it or can see at
/// least one member.
pub async fn fetch_transaction_link(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    id: Uuid,
) -> async_graphql::Result<Option<TransactionLink>> {
    let Some(view) = links_by_ids(db, &user.account_ids, &[id]).await?.remove(&id) else {
        return Ok(None);
    };
    let owner = transaction_link::Entity::find_by_id(id)
        .one(db)
        .await?
        .is_some_and(|l| l.owner_user_id == Some(user.user_id));
    let sees_a_member = view.members.iter().any(|m| m.amount.is_some());
    Ok((owner || sees_a_member).then_some(view))
}

/// `transactionLinks`: links with at least one member in the caller's
/// accounts, most recently edited first.
pub async fn fetch_transaction_links(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    limit: i32,
    offset: i32,
) -> async_graphql::Result<Vec<TransactionLink>> {
    if user.account_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mine = Query::select()
        .column(transaction::Column::Id)
        .from(transaction::Entity)
        .and_where(transaction::Column::AccountId.is_in(user.account_ids.clone()))
        .to_owned();
    let link_ids: Vec<Uuid> = transaction_link_member::Entity::find()
        .filter(transaction_link_member::Column::TransactionId.in_subquery(mine))
        .select_only()
        .column(transaction_link_member::Column::LinkId)
        .distinct()
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    let rows = transaction_link::Entity::find()
        .filter(transaction_link::Column::Id.is_in(link_ids))
        .order_by_desc(transaction_link::Column::Revision)
        .order_by_asc(transaction_link::Column::Id)
        .limit(limit.clamp(1, 200) as u64)
        .offset(offset.max(0) as u64)
        .all(db)
        .await?;
    let order: Vec<Uuid> = rows.iter().map(|l| l.id).collect();
    let mut views = links_by_ids(db, &user.account_ids, &order).await?;
    Ok(order.iter().filter_map(|id| views.remove(id)).collect())
}

/// How well `candidate` fits `base` as the other side of a link, 0..1.
/// Amount closeness counts for more than date proximity: a partial
/// reimbursement is common, but a reimbursement of an unrelated sum is not.
/// Pure.
pub(crate) fn candidate_score(
    base_amount: Decimal,
    base_date: chrono::NaiveDate,
    candidate_amount: Decimal,
    candidate_date: chrono::NaiveDate,
) -> f64 {
    use rust_decimal::prelude::ToPrimitive;
    let (a, b) = (base_amount.abs(), candidate_amount.abs());
    let (small, large) = if a < b { (a, b) } else { (b, a) };
    let amount_fit = if large.is_zero() { 0.0 } else { (small / large).to_f64().unwrap_or(0.0) };
    let days = (candidate_date - base_date).num_days().unsigned_abs() as f64;
    let date_fit = 1.0 / (1.0 + days / 30.0);
    0.6 * amount_fit + 0.4 * date_fit
}

/// How far either side of the base transaction's date a suggestion is
/// looked for when the user has typed nothing: a reimbursement trails its
/// expense by weeks, rarely by more than half a year.
const CANDIDATE_WINDOW_DAYS: i64 = 180;
/// Rows ranked per request; the SQL pre-sorts by date distance.
const CANDIDATE_POOL: u64 = 300;

/// `linkCandidates`: transactions that could be the other side of a link
/// from `transaction_id`, best first. Opposite sign, same currency, not
/// already linked, within the caller's accounts. With no `search` only the
/// transactions near it in time are considered; with one, the whole history
/// is searched (case-insensitive over counterparty and description).
pub async fn fetch_link_candidates(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    transaction_id: Uuid,
    search: Option<String>,
    limit: i32,
) -> async_graphql::Result<Vec<LinkCandidate>> {
    let scoped_ids = scoped_account_ids(user, None)?;
    let base = crate::graphql::labels::load_scoped_transaction(db, &scoped_ids, transaction_id).await?;
    if base.amount.is_zero() {
        return Err(validation_error("a zero-amount transaction cannot be linked"));
    }
    let search = search.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let filter = TransactionFilter {
        search: search.clone(),
        direction: Some(if base.amount < Decimal::ZERO { Direction::Income } else { Direction::Spending }),
        ..Default::default()
    };
    let mut condition = build_condition_with_category_filters(db, &scoped_ids, &filter).await?;
    condition = condition
        .add(transaction::Column::Id.ne(base.id))
        .add(transaction::Column::Currency.eq(base.currency.clone()))
        .add(Expr::cust(
            "NOT EXISTS (SELECT 1 FROM transaction_link_member m WHERE m.transaction_id = transaction.id)",
        ));
    if search.is_none() {
        let window = chrono::Duration::days(CANDIDATE_WINDOW_DAYS);
        condition = condition
            .add(transaction::Column::BookingDate.gte(base.booking_date - window))
            .add(transaction::Column::BookingDate.lte(base.booking_date + window));
    }
    let rows = transaction::Entity::find()
        .filter(condition)
        .order_by(
            Expr::cust_with_values("ABS(transaction.booking_date - $1)", vec![sea_orm::Value::from(base.booking_date)]),
            Order::Asc,
        )
        .order_by_asc(transaction::Column::Id)
        .limit(CANDIDATE_POOL)
        .all(db)
        .await?;

    let mut scored: Vec<(f64, transaction::Model)> = rows
        .into_iter()
        .map(|t| (candidate_score(base.amount, base.booking_date, t.amount, t.booking_date), t))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
    scored.truncate(limit.clamp(1, 50) as usize);
    Ok(scored
        .into_iter()
        .map(|(score, t)| LinkCandidate { transaction: to_graphql_transaction(t), score })
        .collect())
}

/// `reimbursementSummary`: for the expense-side transactions the filter
/// matches, how much of them linked reimbursements cover. The period's own
/// totals are untouched; this is the netted view next to them.
pub async fn fetch_reimbursement_summary(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
) -> async_graphql::Result<ReimbursementSummary> {
    let zero = || GqlDecimal(Decimal::ZERO);
    if scoped_ids.is_empty() {
        return Ok(ReimbursementSummary {
            expense_total: zero(),
            reimbursed: zero(),
            net: zero(),
            linked_count: 0,
            partially_reimbursed_count: 0,
        });
    }
    let condition = build_condition_with_category_filters(db, scoped_ids, filter)
        .await?
        .add(Expr::cust(
            "EXISTS (SELECT 1 FROM transaction_link_member m WHERE m.transaction_id = transaction.id AND m.role = 'expense')",
        ));
    let matched: Vec<(Uuid, Decimal)> = transaction::Entity::find()
        .filter(condition)
        .select_only()
        .columns([transaction::Column::Id, transaction::Column::Amount])
        .into_tuple()
        .all(db)
        .await?;
    let ids: Vec<Uuid> = matched.iter().map(|(id, _)| *id).collect();
    let links = links_for_transactions(db, scoped_ids, &ids).await?;

    let (mut expense_total, mut reimbursed) = (Decimal::ZERO, Decimal::ZERO);
    let (mut linked, mut partial) = (0, 0);
    for (id, amount) in &matched {
        let Some(member) = links
            .get(id)
            .and_then(|l| l.members.iter().find(|m| m.transaction_id.0 == *id && m.role == TransactionLinkRole::Expense))
        else {
            continue;
        };
        linked += 1;
        expense_total += amount.abs();
        reimbursed += member.allocated.0;
        if member.remaining.0 > Decimal::ZERO {
            partial += 1;
        }
    }
    Ok(ReimbursementSummary {
        expense_total: GqlDecimal(expense_total),
        reimbursed: GqlDecimal(reimbursed),
        net: GqlDecimal(expense_total - reimbursed),
        linked_count: linked,
        partially_reimbursed_count: partial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn date(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn tx(amount: &str, currency: &str) -> transaction::Model {
        transaction::Model {
            id: Uuid::new_v4(),
            account_id: Uuid::new_v4(),
            source: "test".into(),
            external_id: Uuid::new_v4().to_string(),
            booking_date: date(2024, 9, 10),
            valuta_date: None,
            booking_status: "BOOKED".into(),
            amount: d(amount),
            currency: currency.into(),
            counterparty_name: Some("Praxis Dr. Mueller".into()),
            counterparty_iban: None,
            description: None,
            transaction_type: None,
            raw_payload: serde_json::json!({}),
            origin: "test".into(),
            imported_at: Utc::now().into(),
            updated_at: Utc::now().into(),
            counterparty_key: None,
        }
    }

    fn link_row() -> transaction_link::Model {
        transaction_link::Model {
            id: Uuid::new_v4(),
            kind: "reimbursement".into(),
            owner_user_id: None,
            note: None,
            revision: Utc::now().into(),
        }
    }

    fn member(link: &transaction_link::Model, tx: &transaction::Model, role: &str) -> transaction_link_member::Model {
        transaction_link_member::Model { link_id: link.id, transaction_id: tx.id, role: role.into() }
    }

    #[test]
    fn build_link_reports_a_partial_reimbursement_and_drops_a_missing_member_from_the_figures() {
        let (bill, back, gone) = (tx("-1000", "EUR"), tx("600", "EUR"), tx("400", "EUR"));
        let link = link_row();
        let members = [member(&link, &bill, "expense"), member(&link, &back, "offset"), member(&link, &gone, "offset")];
        // `gone` is not in the visible set.
        let visible: HashMap<Uuid, transaction::Model> =
            [bill.clone(), back.clone()].into_iter().map(|t| (t.id, t)).collect();
        let view = build_link(&link, &members, &visible);
        assert_eq!(view.status, TransactionLinkStatus::Partial);
        assert_eq!(view.missing_members, 1);
        assert_eq!(
            (view.expense_total.0, view.offset_total.0, view.reimbursed.0, view.net.0),
            (d("1000"), d("600"), d("600"), d("400"))
        );
        let gone_view = view.members.iter().find(|m| m.transaction_id.0 == gone.id).unwrap();
        assert!(gone_view.amount.is_none() && gone_view.booking_date.is_none());
    }

    #[test]
    fn build_link_with_nothing_visible_is_incomplete() {
        let bill = tx("-10", "EUR");
        let link = link_row();
        let view = build_link(&link, &[member(&link, &bill, "expense")], &HashMap::new());
        assert_eq!(view.status, TransactionLinkStatus::Incomplete);
        assert_eq!(view.missing_members, 1);
        assert_eq!(view.net.0, d("0"));
    }

    #[test]
    fn check_members_accepts_a_well_formed_many_to_one() {
        let (a, b, back) = (tx("-10", "EUR"), tx("-20", "EUR"), tx("30", "EUR"));
        assert_eq!(check_members(LinkKind::Reimbursement, &[a, b], &[back]), Ok(()));
    }

    #[test]
    fn check_members_rejects_a_side_with_the_wrong_sign() {
        let (a, back) = (tx("-10", "EUR"), tx("-30", "EUR"));
        let err = check_members(LinkKind::Reimbursement, std::slice::from_ref(&a), &[back]).unwrap_err();
        assert!(err.contains("offset side must be money received"), "{err}");
        let err = check_members(LinkKind::Reimbursement, &[tx("10", "EUR")], &[tx("5", "EUR")]).unwrap_err();
        assert!(err.contains("expense side must be money paid out"), "{err}");
    }

    #[test]
    fn check_members_rejects_empty_sides_overlap_and_mixed_currencies() {
        let (a, back) = (tx("-10", "EUR"), tx("30", "EUR"));
        assert!(check_members(LinkKind::Reimbursement, &[], std::slice::from_ref(&back)).is_err());
        assert!(check_members(LinkKind::Reimbursement, std::slice::from_ref(&a), &[]).is_err());
        let overlap = transaction::Model { id: a.id, ..back.clone() };
        assert!(check_members(LinkKind::Reimbursement, std::slice::from_ref(&a), &[overlap]).is_err());
        let usd = tx("30", "USD");
        assert!(check_members(LinkKind::Reimbursement, &[a], &[usd]).unwrap_err().contains("currency"));
    }

    #[test]
    fn check_members_caps_the_size_of_a_link() {
        let expense: Vec<_> = (0..MAX_LINK_MEMBERS).map(|_| tx("-1", "EUR")).collect();
        assert!(check_members(LinkKind::Reimbursement, &expense, &[tx("50", "EUR")]).is_err());
    }

    #[test]
    fn note_is_trimmed_blank_means_none_and_long_is_rejected() {
        assert_eq!(normalize_note(Some("  dentist  ".into())).unwrap(), Some("dentist".into()));
        assert_eq!(normalize_note(Some("   ".into())).unwrap(), None);
        assert_eq!(normalize_note(None).unwrap(), None);
        assert!(normalize_note(Some("x".repeat(MAX_NOTE_CHARS + 1))).is_err());
    }

    #[test]
    fn dedupe_keeps_first_occurrences_in_order() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(dedupe(&[GqlUuid(a), GqlUuid(b), GqlUuid(a)]), vec![a, b]);
    }

    #[test]
    fn candidate_score_prefers_the_closer_amount_and_the_nearer_date() {
        let base = (d("-1000"), date(2024, 9, 10));
        let exact_near = candidate_score(base.0, base.1, d("1000"), date(2024, 9, 20));
        let partial_near = candidate_score(base.0, base.1, d("600"), date(2024, 9, 20));
        let exact_far = candidate_score(base.0, base.1, d("1000"), date(2025, 3, 10));
        assert!(exact_near > partial_near, "closer amount wins");
        assert!(exact_near > exact_far, "nearer date wins");
        assert!((0.0..=1.0).contains(&exact_near));
        // Symmetric in which side is the base.
        let flipped = candidate_score(d("1000"), date(2024, 9, 20), d("-1000"), date(2024, 9, 10));
        assert!((exact_near - flipped).abs() < 1e-9);
    }
}
