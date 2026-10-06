//! Query root (§5). Field *signatures* are the frozen SDL contract WP0
//! exports; resolver bodies here are stubs for WP4 to replace — real
//! implementations need the auth context (§4) and projector-filled tables
//! that don't exist yet at this point in the iteration.

use async_graphql::{Context, ErrorExtensions, Object, Result as GqlResult};

use crate::graphql::types::{
    Account, CashflowGraph, CashflowGraphInput, CashflowSummary, Granularity, Me, PageInput,
    TransactionFilter, TransactionPage,
};

pub struct QueryRoot;

/// Every resolver but `me`/`login` is unimplemented here; WP4 wires them to
/// the auth context and the projected read model.
fn not_implemented(field: &str) -> async_graphql::Error {
    async_graphql::Error::new(format!("{field}: not implemented (WP4)"))
        .extend_with(|_, e| e.set("code", "NOT_IMPLEMENTED"))
}

#[Object(name = "Query")]
impl QueryRoot {
    /// Unauthenticated field (§4/§5): no session support exists yet, so this
    /// always returns `null` rather than guessing at a user.
    async fn me(&self, _ctx: &Context<'_>) -> GqlResult<Option<Me>> {
        Ok(None)
    }

    async fn accounts(&self, _ctx: &Context<'_>) -> GqlResult<Vec<Account>> {
        Err(not_implemented("accounts"))
    }

    async fn transactions(
        &self,
        _ctx: &Context<'_>,
        _filter: Option<TransactionFilter>,
        _page: Option<PageInput>,
    ) -> GqlResult<TransactionPage> {
        Err(not_implemented("transactions"))
    }

    async fn cashflow_summary(
        &self,
        _ctx: &Context<'_>,
        _filter: TransactionFilter,
        _granularity: Granularity,
    ) -> GqlResult<CashflowSummary> {
        Err(not_implemented("cashflowSummary"))
    }

    async fn cashflow_graph(
        &self,
        _ctx: &Context<'_>,
        _filter: TransactionFilter,
        _grouping: Option<CashflowGraphInput>,
    ) -> GqlResult<CashflowGraph> {
        Err(not_implemented("cashflowGraph"))
    }
}
