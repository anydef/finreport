//! Iteration 4 §2.1: the user's `finreport.goal` topic — a goal is a human
//! decision about how to read one's own money, keyed by its own id,
//! last-writer-wins, the same shape as `finreport.rule`. Frozen by WP0;
//! projected by `projection::goals` (WP-A), written by the GraphQL goal
//! mutations (WP-B).

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One record per goal, keyed by the goal's own UUID as a string (not a
/// deterministic UUIDv5 — a goal has no natural key, so ids are random and
/// minted by `createGoal`). Compacted, partitions 1, RF 1, `prevent_destroy`,
/// iteration-1 envelope headers with `origin = user`; a tombstone deletes the
/// goal (§2.1).
pub const TOPIC_GOAL: &str = "finreport.goal";

/// Current payload schema version for [`GoalRecord`] (§2.1).
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// `type` (§2.1). Mirrors `goal.goal_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalType {
    /// Spending in scope must stay at or under the amount.
    SpendingLimit,
    /// Saving in scope must reach at least the amount.
    SavingTarget,
}

/// `scope.combine` / `scope.tag_combine` (§2.1). Mirrors
/// `goal.scope_combine` / `goal.scope_tag_combine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Combine {
    All,
    Any,
}

/// What a goal is measured against (§2.1, §3.2). Mirrors the
/// `goal.scope_*` columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalScope {
    /// Slugs, not ids, so renaming a category never invalidates a goal; each
    /// includes its descendants.
    pub category_slugs: Vec<String>,
    pub tags: Vec<String>,
    /// How the category condition and the tag condition join.
    pub combine: Combine,
    /// How the listed tags join each other.
    pub tag_combine: Combine,
}

/// `period.cadence` (§2.1); recurring periods only. Mirrors
/// `goal.period_cadence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cadence {
    Monthly,
    Quarterly,
    Yearly,
}

/// A goal's period, tagged by `kind` (§2.1): `cadence` belongs to recurring
/// goals only, `start_date`/`end_date` to fixed ones. Flat rather than an
/// enum so the wire shape matches the spec's single `period` object exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalPeriod {
    pub kind: PeriodKind,
    /// `Some` for `recurring` only.
    #[serde(default)]
    pub cadence: Option<Cadence>,
    /// `Some` for `fixed` only.
    #[serde(default)]
    pub start_date: Option<NaiveDate>,
    /// `fixed` only; `None` = open-ended.
    #[serde(default)]
    pub end_date: Option<NaiveDate>,
}

/// `period.kind` (§2.1). Mirrors `goal.period_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeriodKind {
    Recurring,
    Fixed,
}

/// Published on [`TOPIC_GOAL`] (§2.1). Whole-state: the topic compacts per
/// goal, so every mutation is read-modify-write and publishes the complete
/// record with a fresh `revision`. Mirrors the `goal` columns (§2.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalRecord {
    pub schema_version: u32,
    pub id: Uuid,
    pub owner_user_id: Uuid,
    pub name: String,
    #[serde(rename = "type")]
    pub goal_type: GoalType,
    /// Positive magnitude, `NUMERIC(20,4)`; serialized as a string
    /// (`"200.0000"`) so no precision is lost on the wire.
    pub amount: Decimal,
    pub currency: String,
    pub scope: GoalScope,
    pub period: GoalPeriod,
    pub archived: bool,
    /// RFC 3339; last-writer-wins (iteration 2 §2.1).
    pub revision: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn spec_example_json() -> &'static str {
        r#"{
            "schema_version": 1,
            "id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
            "owner_user_id": "0b8f6c3e-6a0f-4b64-9f6b-1b0e1f2d3a4c",
            "name": "Hobbies",
            "type": "spending_limit",
            "amount": "200.0000",
            "currency": "EUR",
            "scope": {
                "category_slugs": ["leisure.hobbies"],
                "tags": ["hobby"],
                "combine": "any",
                "tag_combine": "all"
            },
            "period": {
                "kind": "recurring",
                "cadence": "monthly",
                "start_date": null,
                "end_date": null
            },
            "archived": false,
            "revision": "2027-01-01T10:00:00Z"
        }"#
    }

    #[test]
    fn parses_the_spec_example_and_round_trips() {
        let record: GoalRecord = serde_json::from_str(spec_example_json()).unwrap();
        assert_eq!(record.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(record.goal_type, GoalType::SpendingLimit);
        assert_eq!(record.amount, Decimal::from_str("200.0000").unwrap());
        assert_eq!(record.scope.combine, Combine::Any);
        assert_eq!(record.scope.tag_combine, Combine::All);
        assert_eq!(record.period.kind, PeriodKind::Recurring);
        assert_eq!(record.period.cadence, Some(Cadence::Monthly));

        let json = serde_json::to_string(&record).unwrap();
        let again: GoalRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, again);
    }

    #[test]
    fn serializes_amount_as_a_string_and_type_under_its_wire_name() {
        let record: GoalRecord = serde_json::from_str(spec_example_json()).unwrap();
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["amount"], "200.0000");
        assert_eq!(value["type"], "spending_limit");
        assert!(value.get("goal_type").is_none());
    }

    #[test]
    fn fixed_period_with_open_end_and_omitted_cadence_parses() {
        let json = spec_example_json().replace(
            r#""kind": "recurring",
                "cadence": "monthly",
                "start_date": null,"#,
            r#""kind": "fixed",
                "start_date": "2026-01-01","#,
        );
        let record: GoalRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record.period.kind, PeriodKind::Fixed);
        assert_eq!(record.period.cadence, None);
        assert_eq!(
            record.period.start_date,
            NaiveDate::from_ymd_opt(2026, 1, 1)
        );
        assert_eq!(record.period.end_date, None);
    }

    #[test]
    fn saving_target_uses_snake_case() {
        let json = spec_example_json().replace("spending_limit", "saving_target");
        let record: GoalRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record.goal_type, GoalType::SavingTarget);
    }
}
