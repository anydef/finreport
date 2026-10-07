//! Iteration 4 §4: the goals GraphQL surface — `goals`, `goal`,
//! `goalProgress`, `goalTransactions` and the `createGoal`/`updateGoal`/
//! `archiveGoal` mutations.
//!
//! **WP0 stubs.** The types and the resolver signatures here are the frozen
//! contract; every function returns `NOT_IMPLEMENTED` naming the work package
//! that owns its body (`WP-B`, with `WP-A` supplying the evaluation they call
//! into). `queries.rs`/`mutations.rs` delegate to these free functions, the
//! same arrangement iteration 3 used for `insights.rs`.

use async_graphql::{Enum, InputObject, SimpleObject};

use crate::graphql::scalars::{Date, Decimal, Uuid};
use crate::graphql::types::{not_implemented_in, Category, PageInput, TransactionPage};

const ITERATION: u8 = 4;
const OWNER: &str = "WP-B";

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum GoalType {
    SpendingLimit,
    SavingTarget,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum GoalPeriodKind {
    Recurring,
    Fixed,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum GoalCadence {
    Monthly,
    Quarterly,
    Yearly,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ScopeCombine {
    All,
    Any,
}

#[derive(SimpleObject)]
pub struct GoalScope {
    /// Resolved from slugs; archived ones are still listed.
    pub categories: Vec<Category>,
    pub tags: Vec<String>,
    pub combine: ScopeCombine,
    pub tag_combine: ScopeCombine,
}

#[derive(SimpleObject)]
pub struct Goal {
    pub id: Uuid,
    pub name: String,
    #[graphql(name = "type")]
    pub goal_type: GoalType,
    pub amount: Decimal,
    pub currency: String,
    pub scope: GoalScope,
    pub period_kind: GoalPeriodKind,
    /// `null` for `FIXED`.
    pub cadence: Option<GoalCadence>,
    /// `null` for `RECURRING`.
    pub start_date: Option<Date>,
    /// `null` for `RECURRING` or an open-ended `FIXED`.
    pub end_date: Option<Date>,
    pub archived: bool,
}

#[derive(SimpleObject)]
pub struct GoalBucket {
    pub start: Date,
    pub end: Date,
    pub label: String,
    /// Positive magnitude.
    pub total: Decimal,
    /// Held-for-review, excluded from `total`.
    pub pending: Decimal,
    /// `amount - total`; negative when over.
    pub remaining: Decimal,
    /// `<= amount` for a limit, `>= amount` for a target.
    pub met: bool,
    pub in_progress: bool,
}

#[derive(SimpleObject)]
pub struct GoalProgress {
    pub goal: Goal,
    pub buckets: Vec<GoalBucket>,
    /// Across every bucket.
    pub total: Decimal,
    pub pending: Decimal,
    /// Over completed buckets only.
    pub average_per_period: Decimal,
    pub currency: String,
}

/// Mirrors the `finreport.goal` event's scope and period (§2.1). Validated
/// at mutation time (§4): amount > 0; a non-empty scope; `RECURRING` requires
/// `cadence` and rejects `startDate`/`endDate`; `FIXED` requires `startDate`
/// and rejects `cadence`; `endDate >= startDate`; every category slug exists;
/// tags normalized with iteration 3's rules.
#[derive(InputObject)]
pub struct GoalInput {
    pub name: String,
    #[graphql(name = "type")]
    pub goal_type: GoalType,
    pub amount: Decimal,
    #[graphql(default = "EUR")]
    pub currency: String,
    #[graphql(default)]
    pub category_slugs: Vec<String>,
    #[graphql(default)]
    pub tags: Vec<String>,
    /// How the category and tag conditions join; ignored when the scope has
    /// only one of them.
    #[graphql(default_with = "ScopeCombine::All")]
    pub combine: ScopeCombine,
    /// How the listed tags join each other.
    #[graphql(default_with = "ScopeCombine::All")]
    pub tag_combine: ScopeCombine,
    pub period_kind: GoalPeriodKind,
    pub cadence: Option<GoalCadence>,
    pub start_date: Option<Date>,
    pub end_date: Option<Date>,
}

pub async fn goals(include_archived: bool) -> async_graphql::Result<Vec<Goal>> {
    let _ = include_archived;
    Err(not_implemented_in(ITERATION, "goals", OWNER))
}

pub async fn goal(id: Uuid) -> async_graphql::Result<Option<Goal>> {
    let _ = id;
    Err(not_implemented_in(ITERATION, "goal", OWNER))
}

pub async fn goal_progress(
    id: Uuid,
    start_date: Option<Date>,
    end_date: Option<Date>,
) -> async_graphql::Result<GoalProgress> {
    let _ = (id, start_date, end_date);
    Err(not_implemented_in(ITERATION, "goalProgress", OWNER))
}

pub async fn goal_transactions(
    id: Uuid,
    start_date: Date,
    end_date: Date,
    page: Option<PageInput>,
) -> async_graphql::Result<TransactionPage> {
    let _ = (id, start_date, end_date, page);
    Err(not_implemented_in(ITERATION, "goalTransactions", OWNER))
}

pub async fn create_goal(input: GoalInput) -> async_graphql::Result<Goal> {
    let _ = input;
    Err(not_implemented_in(ITERATION, "createGoal", OWNER))
}

pub async fn update_goal(id: Uuid, input: GoalInput) -> async_graphql::Result<Goal> {
    let _ = (id, input);
    Err(not_implemented_in(ITERATION, "updateGoal", OWNER))
}

pub async fn archive_goal(id: Uuid) -> async_graphql::Result<Goal> {
    let _ = id;
    Err(not_implemented_in(ITERATION, "archiveGoal", OWNER))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(err: &async_graphql::Error) -> Option<String> {
        err.extensions
            .as_ref()
            .and_then(|e| e.get("code"))
            .map(|v| v.to_string())
    }

    #[tokio::test]
    async fn every_stub_reports_not_implemented_naming_its_field_and_owner() {
        let id = Uuid(uuid::Uuid::nil());
        let date = Date(chrono::NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
        let errors = [
            ("goals", goals(false).await.err()),
            ("goal", goal(id).await.err()),
            ("goalProgress", goal_progress(id, None, None).await.err()),
            (
                "goalTransactions",
                goal_transactions(id, date, date, None).await.err(),
            ),
            ("archiveGoal", archive_goal(id).await.err()),
        ];
        for (field, err) in errors {
            let err = err.unwrap_or_else(|| panic!("{field} should be a stub"));
            assert_eq!(code(&err).as_deref(), Some("\"NOT_IMPLEMENTED\""), "{field}");
            assert!(err.message.contains(field), "{field}: {}", err.message);
            assert!(err.message.contains(OWNER), "{field}: {}", err.message);
            assert!(err.message.contains("iteration-4"), "{field}: {}", err.message);
        }
    }
}
