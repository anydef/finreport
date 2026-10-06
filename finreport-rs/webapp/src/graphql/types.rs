//! GraphQL object/input/enum types for the §5 SDL, beyond the scalars
//! (`scalars.rs`). Field shapes here are the frozen contract; resolver bodies
//! in `queries.rs`/`mutations.rs` are WP4's to fill in.

use async_graphql::{Enum, InputObject, SimpleObject, ID};

use crate::graphql::scalars::{Date, Decimal, Uuid};

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
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum Direction {
    Income,
    Spending,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum Granularity {
    Day,
    Week,
    Month,
}

#[derive(InputObject)]
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
}

#[derive(InputObject)]
pub struct PageInput {
    /// Clamped to `1..=200`.
    #[graphql(default = 50)]
    pub limit: i32,
    #[graphql(default = 0)]
    pub offset: i32,
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
