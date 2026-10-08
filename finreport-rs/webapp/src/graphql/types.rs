//! GraphQL object/input/enum types for the §5 SDL, beyond the scalars
//! (`scalars.rs`). Field shapes here are the frozen contract; resolver bodies
//! in `queries.rs`/`mutations.rs` are WP4's to fill in.

use async_graphql::{ComplexObject, Enum, ErrorExtensions, InputObject, SimpleObject, ID};

use crate::graphql::scalars::{Date, Decimal, Json, Uuid};

/// A frozen (WP0) resolver stub's error, shared by every iteration-2 field
/// that isn't implemented yet. `extensions.code = "NOT_IMPLEMENTED"` so a
/// client (or an integration test) can tell this apart from a real failure;
/// `wp` names the work package that owns the real implementation.
#[allow(dead_code)]
pub(crate) fn not_implemented(field: &str, wp: u8) -> async_graphql::Error {
    async_graphql::Error::new(format!(
        "{field} is not implemented yet (WP{wp}, see docs/specs/iteration-2.md)"
    ))
    .extend_with(|_, e| e.set("code", "NOT_IMPLEMENTED"))
}

/// Iteration-N counterpart of [`not_implemented`]: `iteration` selects which
/// `docs/specs/iteration-N.md` the error points at, `wp` (a string, since
/// later iterations name packages `WP-A`, `WP-B`, ...) the owning work
/// package. Same `extensions.code = "NOT_IMPLEMENTED"`.
pub(crate) fn not_implemented_in(iteration: u8, field: &str, wp: &str) -> async_graphql::Error {
    async_graphql::Error::new(format!(
        "{field} is not implemented yet ({wp}, see docs/specs/iteration-{iteration}.md)"
    ))
    .extend_with(|_, e| e.set("code", "NOT_IMPLEMENTED"))
}

/// Iteration-3 stubs: WP-B has since implemented all five call sites, so
/// nothing references this anymore — kept in case it is needed again.
#[allow(dead_code)]
pub(crate) fn not_implemented_iter3(field: &str, wp: &str) -> async_graphql::Error {
    not_implemented_in(3, field, wp)
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[derive(InputObject)]
pub struct LoginInput {
    pub username: String,
    pub password: String,
}

#[derive(SimpleObject)]
pub struct Me {
    pub id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub is_admin: bool,
}

// ---------------------------------------------------------------------------
// Accounts & balances
// ---------------------------------------------------------------------------

#[derive(SimpleObject)]
pub struct Account {
    pub id: Uuid,
    /// `"comdirect"`.
    pub source: String,
    pub external_id: String,
    pub display_id: Option<String>,
    pub account_type: Option<String>,
    pub iban: Option<String>,
    pub bic: Option<String>,
    pub institute: Option<String>,
    /// Login label, display only.
    pub label: Option<String>,
    pub currency: String,
    pub latest_balance: Option<Balance>,
}

#[derive(SimpleObject)]
pub struct Balance {
    pub date: Date,
    pub amount: Decimal,
    pub currency: String,
}

// ---------------------------------------------------------------------------
// Transactions
// ---------------------------------------------------------------------------

#[derive(SimpleObject)]
#[graphql(complex)]
pub struct Transaction {
    pub id: Uuid,
    pub account_id: Uuid,
    pub source: String,
    pub external_id: String,
    pub booking_date: Date,
    pub valuta_date: Option<Date>,
    pub booking_status: String,
    pub amount: Decimal,
    pub currency: String,
    pub counterparty_name: Option<String>,
    pub counterparty_iban: Option<String>,
    pub description: Option<String>,
    pub transaction_type: Option<String>,
    /// Normalised counterparty (`transaction.counterparty_key`), the same key
    /// rule learning and recurring detection use; filter by it with
    /// `TransactionFilter.counterpartyKeys`. `null` until the labeler has
    /// normalised the row.
    pub counterparty_key: Option<String>,
}

/// Iteration 2 (§5): `label`/`splits` are resolver-computed, not eagerly
/// loaded with the rest of `Transaction`'s fields, since they live in
/// separate projections (§3) that may not exist for every transaction (a
/// `null` label means "not labelled yet", distinct from `needsReview`).
#[ComplexObject]
impl Transaction {
    async fn label(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Option<TransactionLabel>> {
        let db: &std::sync::Arc<sea_orm::DatabaseConnection> = ctx.data()?;
        let cache = ctx.data::<crate::graphql::labels::LabelSplitCache>().ok();
        crate::graphql::labels::label_for(db.as_ref(), cache, self.id.0).await
    }

    async fn splits(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Vec<TransactionSplit>> {
        let db: &std::sync::Arc<sea_orm::DatabaseConnection> = ctx.data()?;
        let cache = ctx.data::<crate::graphql::labels::LabelSplitCache>().ok();
        crate::graphql::labels::splits_for(db.as_ref(), cache, self.id.0).await
    }

    /// Iteration 3 §4: sorted, `[]` when untagged.
    async fn tags(&self, ctx: &async_graphql::Context<'_>) -> async_graphql::Result<Vec<String>> {
        let db: &std::sync::Arc<sea_orm::DatabaseConnection> = ctx.data()?;
        crate::graphql::insights::tags_for(db.as_ref(), self.id.0).await
    }

    /// `null` = not an internal transfer (§4).
    async fn transfer(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Option<TransferInfo>> {
        let db: &std::sync::Arc<sea_orm::DatabaseConnection> = ctx.data()?;
        crate::graphql::insights::transfer_for(db.as_ref(), self.id.0).await
    }

    /// Always present; `isRecurring` may be `false` (§4).
    async fn recurring(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<RecurringInfo> {
        let db: &std::sync::Arc<sea_orm::DatabaseConnection> = ctx.data()?;
        crate::graphql::insights::recurring_for(db.as_ref(), self.id.0).await
    }
}

// ---------------------------------------------------------------------------
// Iteration 3: tags, internal transfers, recurring costs (§4)
// ---------------------------------------------------------------------------

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum FlagSource {
    Auto,
    User,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RecurringCadence {
    Monthly,
    Quarterly,
    Yearly,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum TransferMatch {
    Iban,
    AmountDate,
}

/// `counterpartTransactionId` is `null` while the other leg is not
/// projected yet (§4).
#[derive(SimpleObject)]
pub struct TransferInfo {
    pub counterpart_transaction_id: Option<Uuid>,
    pub counterpart_account_id: Option<Uuid>,
    #[graphql(name = "match")]
    pub match_: TransferMatch,
}

/// `source = User` when overridden (§2.2); `seriesId` is `null` when
/// overridden in or out of a series.
#[derive(SimpleObject)]
pub struct RecurringInfo {
    pub is_recurring: bool,
    pub source: FlagSource,
    pub series_id: Option<Uuid>,
    pub cadence: Option<RecurringCadence>,
    pub median_amount: Option<Decimal>,
}

#[derive(SimpleObject)]
pub struct RecurringSeries {
    pub id: Uuid,
    pub counterparty_key: String,
    pub counterparty_name: Option<String>,
    pub direction: Direction,
    pub cadence: RecurringCadence,
    /// Signed.
    pub median_amount: Decimal,
    /// Signed, 4 dp (§3.2).
    pub monthly_equivalent: Decimal,
    pub occurrence_count: i32,
    pub first_date: Date,
    pub last_date: Date,
    pub next_expected_date: Date,
    /// `lastDate` older than cadence + grace.
    pub stale: bool,
}

#[derive(SimpleObject)]
pub struct RecurringOverview {
    pub series: Vec<RecurringSeries>,
    /// Expense series only, positive magnitude.
    pub total_monthly_equivalent: Decimal,
    pub currency: String,
}

#[derive(SimpleObject)]
pub struct TagCount {
    pub tag: String,
    pub transaction_count: i32,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum Direction {
    Income,
    Spending,
}

/// The column a transaction list is ordered by.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum TransactionSortField {
    /// Booking date, then external id.
    BookingDate,
    /// The **signed** amount: DESC runs largest income to largest expense.
    /// (Filters `amountMin`/`amountMax` bound the magnitude; sorting keeps
    /// the sign so income and expense do not interleave.)
    Amount,
    /// Case-insensitive counterparty name. Transactions without one sort
    /// last in both directions.
    CounterpartyName,
    /// Case-insensitive name of the resolved category (a user override
    /// outranks the label). Uncategorised and split transactions (which
    /// have no single category) sort last in both directions.
    Category,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum SortDirection {
    Asc,
    Desc,
}

/// Ordering for a transaction list. Always completed with a deterministic
/// tiebreaker on the transaction id, so paging through equal keys never
/// repeats or drops a row. Omitting it keeps the default: newest booking
/// date first.
#[derive(InputObject, Copy, Clone, Debug)]
pub struct TransactionSort {
    pub field: TransactionSortField,
    #[graphql(default_with = "SortDirection::Desc")]
    pub direction: SortDirection,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum Granularity {
    Day,
    Week,
    Month,
}

#[derive(InputObject, Default)]
pub struct TransactionFilter {
    /// Inclusive; `None` = unbounded.
    pub start_date: Option<Date>,
    /// Inclusive; `None` = unbounded.
    pub end_date: Option<Date>,
    /// `None`/empty = all of the caller's accounts.
    pub account_ids: Option<Vec<Uuid>>,
    /// Case-insensitive substring over `counterpartyName` + `description`.
    pub search: Option<String>,
    /// `None` = both.
    pub direction: Option<Direction>,
    /// Sankey drill-down: OR-ed exact matches.
    pub counterparty_names: Option<Vec<String>>,
    /// `false` selects the "Unknown" node's rows.
    pub has_counterparty: Option<bool>,
    /// Iteration 2 (§5): OR-ed; includes descendants of each slug.
    pub category_slugs: Option<Vec<String>>,
    /// `true` ⇒ no label at all (distinct from `needsReview`).
    pub uncategorized: Option<bool>,
    pub needs_review: Option<bool>,
    pub label_sources: Option<Vec<LabelSource>>,
    /// Iteration 3 §4: AND-ed — the transaction carries all of them.
    pub tags: Option<Vec<String>>,
    /// Matches the *effective* flag (§2.2, §4): auto unless overridden.
    pub recurring: Option<bool>,
    /// `true` = only transfers, `false` = only non-transfers (§4).
    pub transfer: Option<bool>,
    /// Exact transaction ids, OR-ed among themselves and AND-ed with the
    /// other conditions.
    pub transaction_ids: Option<Vec<Uuid>>,
    /// Inclusive bound on the amount's magnitude (absolute value); `None` =
    /// unbounded. Use `direction` to pick income or spending.
    pub amount_min: Option<Decimal>,
    /// Inclusive bound on the amount's magnitude (absolute value); `None` =
    /// unbounded. Use `direction` to pick income or spending.
    pub amount_max: Option<Decimal>,
    /// Normalized counterparty keys (`transaction.counterparty_key`), OR-ed.
    /// Matches variants of one merchant that `counterpartyNames` would miss.
    /// Omitted or empty = unconstrained.
    pub counterparty_keys: Option<Vec<String>>,
}

#[derive(InputObject)]
pub struct PageInput {
    /// Clamped to `1..=200`.
    #[graphql(default = 50)]
    pub limit: i32,
    #[graphql(default = 0)]
    pub offset: i32,
}

impl Default for PageInput {
    /// Mirrors the `@graphql(default*)` values above (see
    /// `CashflowGraphInput`'s `Default` impl for why this is needed).
    fn default() -> Self {
        Self {
            limit: 50,
            offset: 0,
        }
    }
}

#[derive(SimpleObject)]
pub struct TransactionPage {
    pub items: Vec<Transaction>,
    pub total_count: i32,
    pub limit: i32,
    pub offset: i32,
}

// ---------------------------------------------------------------------------
// Cashflow summary
// ---------------------------------------------------------------------------

#[derive(SimpleObject)]
pub struct CashflowSummary {
    pub buckets: Vec<CashflowBucket>,
    pub total: CashflowTotals,
    pub currency: String,
}

#[derive(SimpleObject)]
pub struct CashflowBucket {
    /// Inclusive.
    pub start: Date,
    /// Inclusive.
    pub end: Date,
    /// Sum of `amount > 0`, positive.
    pub income: Decimal,
    /// Sum of `|amount|` for `amount < 0`, positive.
    pub spending: Decimal,
    /// `income - spending`.
    pub net: Decimal,
    pub transaction_count: i32,
}

#[derive(SimpleObject)]
pub struct CashflowTotals {
    pub income: Decimal,
    pub spending: Decimal,
    pub net: Decimal,
    pub transaction_count: i32,
}

// ---------------------------------------------------------------------------
// Sankey-shaped cash flow graph
// ---------------------------------------------------------------------------

/// `CATEGORY` (iteration 2) and `TAG` (iteration 3) are reserved: declared,
/// rejected until implemented, so an old server never silently returns a
/// graph it did not build (§5).
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CashflowDimension {
    IncomeSource,
    Account,
    Outcome,
    Category,
    Tag,
}

fn default_dimensions() -> Vec<CashflowDimension> {
    vec![
        CashflowDimension::IncomeSource,
        CashflowDimension::Account,
        CashflowDimension::Outcome,
    ]
}

#[derive(InputObject)]
pub struct CashflowGraphInput {
    /// Left-to-right dimensions. Iteration 1 accepts only the default and
    /// errors otherwise.
    #[graphql(default_with = "default_dimensions()")]
    pub dimensions: Vec<CashflowDimension>,
    /// Remainder folded into one `Other` node.
    #[graphql(default = 8)]
    pub max_nodes_per_dimension: i32,
}

impl Default for CashflowGraphInput {
    /// Mirrors the `@graphql(default*)` values above — needed because the
    /// *argument* (`grouping: CashflowGraphInput`, not its fields) is what's
    /// optional in the SDL, so a client omitting it entirely skips field
    /// defaults and resolvers see a plain `None`.
    fn default() -> Self {
        Self {
            dimensions: default_dimensions(),
            max_nodes_per_dimension: 8,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CashflowNodeKind {
    IncomeSource,
    Account,
    Spending,
    Net,
    Deficit,
    Other,
    Category,
    Tag,
}

#[derive(SimpleObject)]
pub struct CashflowNode {
    /// Opaque, stable within one response.
    pub id: ID,
    pub label: String,
    pub kind: CashflowNodeKind,
    /// Column index; the server lays out, not the client.
    pub depth: i32,
    /// `max(inflow, outflow)`.
    pub value: Decimal,
    /// e.g. `"account"` | `"category"` — null for synthetic nodes.
    pub ref_type: Option<String>,
    /// Id within `refType`, for drill-down.
    pub ref_id: Option<String>,
}

#[derive(SimpleObject)]
pub struct CashflowLink {
    pub source_id: ID,
    pub target_id: ID,
    /// Always positive.
    pub value: Decimal,
}

#[derive(SimpleObject)]
pub struct CashflowGraph {
    pub nodes: Vec<CashflowNode>,
    pub links: Vec<CashflowLink>,
    pub currency: String,
    /// Echoes what the server applied.
    pub dimensions: Vec<CashflowDimension>,
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// Categories, labels, rules (iteration 2, §5)
// ---------------------------------------------------------------------------

#[derive(Enum, Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum CategoryKind {
    Income,
    Expense,
    Transfer,
    Saving,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum LabelSource {
    User,
    Rule,
    LlmCache,
    Llm,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum LabelStatus {
    Resolved,
    NeedsReview,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ReviewReason {
    Ambiguous,
    NewCategory,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RuleState {
    Active,
    InReview,
    Revoked,
    Rejected,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum RuleOrigin {
    User,
    Learned,
}

#[derive(SimpleObject, Clone)]
pub struct Category {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub kind: CategoryKind,
    pub parent_id: Option<Uuid>,
    pub depth: i32,
    pub archived: bool,
    /// `"seed"` | `"user"` (§3).
    pub origin: String,
}

#[derive(SimpleObject)]
pub struct TransactionLabel {
    /// `null` while `status = NEEDS_REVIEW`.
    pub category: Option<Category>,
    pub source: LabelSource,
    pub rule: Option<Rule>,
    pub confidence: Option<f32>,
    pub status: LabelStatus,
    pub review_reason: Option<ReviewReason>,
    pub proposed_category_path: Option<String>,
    pub reasoning: Option<String>,
}

#[derive(SimpleObject)]
pub struct TransactionSplit {
    pub index: i32,
    pub amount: Decimal,
    pub category: Category,
}

#[derive(SimpleObject, Clone)]
#[graphql(complex)]
pub struct Rule {
    pub id: Uuid,
    pub name: String,
    pub category: Category,
    pub conditions: Json,
    pub priority: i32,
    pub state: RuleState,
    pub origin: RuleOrigin,
    pub auto_approved: bool,
    pub confidence: Option<f32>,
    pub evidence_count: i32,
    pub created_at: crate::graphql::scalars::DateTime,
}

#[ComplexObject]
impl Rule {
    /// How many of the caller's own transactions this rule's `conditions`
    /// match: the rule's reach, counted over every transaction in the
    /// caller's accounts regardless of how it is currently labelled and
    /// regardless of this rule's `state`. It is NOT the number of
    /// transactions this rule is currently the label source for (a more
    /// specific or higher-priority rule, a user override or a split can
    /// win a transaction this rule also matches). A rule with no
    /// conditions matches every transaction. Evaluated by the labeler's own
    /// condition matcher.
    async fn matching_transaction_count(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<i32> {
        crate::graphql::rules::matching_transaction_count(ctx, &self.conditions.0).await
    }
}

#[derive(SimpleObject)]
pub struct CategoryBreakdownRow {
    /// The roll-up level that was requested.
    pub category: Category,
    /// Positive magnitude.
    pub amount: Decimal,
    pub transaction_count: i32,
    /// Of the row's `kind` total, `0..1`.
    pub share: f32,
}

#[derive(SimpleObject)]
pub struct CategoryBreakdown {
    pub rows: Vec<CategoryBreakdownRow>,
    /// Label missing entirely.
    pub uncategorized: Option<CategoryBreakdownRow>,
    /// Held; counted separately, never as spend.
    pub needs_review: Option<CategoryBreakdownRow>,
    pub currency: String,
}

#[derive(SimpleObject)]
pub struct ReviewQueue {
    /// `status = NEEDS_REVIEW`.
    pub transactions: Vec<Transaction>,
    /// `state = IN_REVIEW`.
    pub pending_rules: Vec<Rule>,
    pub total_count: i32,
}

#[derive(InputObject)]
pub struct SplitPartInput {
    pub amount: Decimal,
    pub category_slug: String,
}

#[derive(InputObject)]
pub struct CategoryInput {
    pub slug: String,
    pub name: String,
    pub kind: CategoryKind,
    pub parent_slug: Option<String>,
}

#[derive(InputObject)]
pub struct RuleInput {
    pub name: String,
    pub category_slug: String,
    pub conditions: Json,
    #[graphql(default = 0)]
    pub priority: i32,
}
