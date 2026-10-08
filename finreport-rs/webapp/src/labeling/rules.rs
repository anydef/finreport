//! §2.7 rule matching: AND of conditions, most-specific-active-rule-wins
//! ordering with a deterministic tie-break, invalid regex skipped rather than
//! panicking.
//!
//! WP0 stub — owned by WP2. The WP0 stub's `RuleMatchInput` only carried
//! `counterparty_key`/`description`/`transaction_type`, which cannot match
//! every `RuleConditions` field (`counterparty_iban`, `direction`,
//! `amount_min`/`amount_max`, `account_ids`) — WP2 owns this file's shape
//! (per `labeling/mod.rs`'s "WP2/WP3 own changing these signatures"), so it
//! is extended here with the fields every condition needs.

use regex::RegexBuilder;
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::kafka::labeling::{RuleConditions, RuleRecord, RuleState};

/// Regexes compiled from `description_regex` are size-bounded so a
/// pathological pattern in a stored rule cannot blow up the labeler's
/// memory; the mutation path (WP4) enforces the same limit at write time.
const DESCRIPTION_REGEX_SIZE_LIMIT: usize = 1 << 16;

/// A transaction's matchable fields, as the rules engine sees them —
/// deliberately narrower than the full `TransactionRecord` (§2.5's
/// resolution chain only ever needs these).
#[derive(Debug, Clone)]
pub struct RuleMatchInput<'a> {
    pub counterparty_key: Option<&'a str>,
    pub counterparty_iban: Option<&'a str>,
    pub description: Option<&'a str>,
    pub transaction_type: Option<&'a str>,
    /// Compared case-insensitively against `RuleConditions::direction`
    /// (e.g. `"SPENDING"` / `"INCOME"`, mirroring the GraphQL `Direction`
    /// enum) — the rules engine's own, coarser notion of direction, not
    /// [`super::fingerprint::Direction`].
    pub direction: Option<&'a str>,
    pub amount: Decimal,
    pub account_id: Option<Uuid>,
}

/// Whether `rule.conditions` all match `input` (AND of every *present*
/// condition; an absent condition imposes no constraint). An uncompilable
/// `description_regex` makes the condition **not match** (and is logged)
/// rather than panicking.
pub fn conditions_match(conditions: &RuleConditions, input: &RuleMatchInput<'_>) -> bool {
    let regex = conditions
        .description_regex
        .as_deref()
        .map(compile_description_regex);
    conditions_match_precompiled(conditions, regex.as_ref(), input)
}

/// How many of `inputs` the `conditions` match, ignoring each transaction's
/// current label and the rule's state: the rule's *reach*. Runs the very
/// same [`conditions_match_precompiled`] the labeler's `most_specific_match`
/// does, so the number cannot drift from what the labeler would do; the
/// regex is compiled once rather than per transaction.
///
/// A rule with no conditions at all matches every input (an absent
/// condition imposes no constraint, as in the labeler).
pub fn count_matching<'a>(
    conditions: &RuleConditions,
    inputs: impl IntoIterator<Item = &'a RuleMatchInput<'a>>,
) -> usize {
    let regex = conditions
        .description_regex
        .as_deref()
        .map(compile_description_regex);
    inputs
        .into_iter()
        .filter(|input| conditions_match_precompiled(conditions, regex.as_ref(), input))
        .count()
}

/// `regex` is `conditions.description_regex` already compiled: `None` when
/// the rule has no regex condition, `Some(None)` when it failed to compile.
fn conditions_match_precompiled(
    conditions: &RuleConditions,
    regex: Option<&Option<regex::Regex>>,
    input: &RuleMatchInput<'_>,
) -> bool {
    if let Some(key) = &conditions.counterparty_key
        && input.counterparty_key != Some(key.as_str())
    {
        return false;
    }
    if let Some(iban) = &conditions.counterparty_iban
        && input.counterparty_iban != Some(iban.as_str())
    {
        return false;
    }
    if let Some(pattern) = &conditions.description_regex {
        match regex.and_then(|r| r.as_ref()) {
            Some(re) => {
                if !input.description.is_some_and(|d| re.is_match(d)) {
                    return false;
                }
            }
            None => {
                tracing::warn!(regex = %pattern, "rule skipped: description_regex does not compile");
                return false;
            }
        }
    }
    if let Some(needle) = &conditions.description_contains {
        let haystack = input.description.unwrap_or("").to_lowercase();
        if !haystack.contains(&needle.to_lowercase()) {
            return false;
        }
    }
    if let Some(direction) = &conditions.direction
        && !input
            .direction
            .is_some_and(|d| d.eq_ignore_ascii_case(direction))
    {
        return false;
    }
    if let Some(min) = conditions.amount_min
        && input.amount < min
    {
        return false;
    }
    if let Some(max) = conditions.amount_max
        && input.amount > max
    {
        return false;
    }
    if let Some(account_ids) = &conditions.account_ids
        && !input.account_id.is_some_and(|id| account_ids.contains(&id))
    {
        return false;
    }
    true
}

/// Compiles `pattern` with the §2.7 size limit; `None` on any compile
/// failure (oversized or invalid) so the caller can skip-and-log instead of
/// panicking.
fn compile_description_regex(pattern: &str) -> Option<regex::Regex> {
    RegexBuilder::new(pattern)
        .size_limit(DESCRIPTION_REGEX_SIZE_LIMIT)
        .build()
        .ok()
}

/// §2.7 specificity score: the number of present conditions, with
/// `counterparty_iban` and `counterparty_key` weighted 2 each and *a bounded
/// amount range* (both `amount_min` and `amount_max` present) weighted 1 as
/// one unit; a lone `amount_min` or `amount_max` is an ordinary
/// weight-1 condition.
fn specificity(conditions: &RuleConditions) -> u32 {
    let mut score = 0;
    if conditions.counterparty_key.is_some() {
        score += 2;
    }
    if conditions.counterparty_iban.is_some() {
        score += 2;
    }
    if conditions.description_regex.is_some() {
        score += 1;
    }
    if conditions.description_contains.is_some() {
        score += 1;
    }
    if conditions.direction.is_some() {
        score += 1;
    }
    match (conditions.amount_min, conditions.amount_max) {
        (Some(_), Some(_)) => score += 1,
        (Some(_), None) | (None, Some(_)) => score += 1,
        (None, None) => {}
    }
    if conditions
        .account_ids
        .as_ref()
        .is_some_and(|ids| !ids.is_empty())
    {
        score += 1;
    }
    score
}

/// Finds the most specific *active* rule among `rules` whose conditions all
/// match `input`, ordered by (a) `priority` descending, (b) [`specificity`]
/// descending, (c) `id` ascending — (c) exists only so the outcome is
/// deterministic, never decided by row order.
pub fn most_specific_match<'a>(
    rules: &'a [RuleRecord],
    input: &RuleMatchInput<'_>,
) -> Option<&'a RuleRecord> {
    rules
        .iter()
        .filter(|rule| rule.state == RuleState::Active)
        .filter(|rule| conditions_match(&rule.conditions, input))
        .max_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| specificity(&a.conditions).cmp(&specificity(&b.conditions)))
                // Tie-break on id ascending: in a max-by, the *smaller* id
                // must compare Greater so it is the one kept as the max.
                .then_with(|| b.id.cmp(&a.id))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kafka::labeling::{RuleOrigin, RuleState};
    use chrono::Utc;

    fn rule(id_byte: u8, priority: i32, conditions: RuleConditions, state: RuleState) -> RuleRecord {
        RuleRecord {
            schema_version: 1,
            id: Uuid::from_bytes([id_byte; 16]),
            name: format!("rule-{id_byte}"),
            category_slug: "food.groceries".to_string(),
            conditions,
            priority,
            state,
            origin: RuleOrigin::User,
            auto_approved: false,
            user_touched: true,
            confidence: None,
            evidence: None,
            created_at: Utc::now(),
            revision: Utc::now(),
        }
    }

    fn input<'a>(counterparty_key: Option<&'a str>) -> RuleMatchInput<'a> {
        RuleMatchInput {
            counterparty_key,
            counterparty_iban: None,
            description: None,
            transaction_type: None,
            direction: None,
            amount: Decimal::from(-10),
            account_id: None,
        }
    }

    #[test]
    fn and_of_conditions_requires_every_present_field_to_match() {
        let conditions = RuleConditions {
            counterparty_key: Some("lidl".to_string()),
            direction: Some("SPENDING".to_string()),
            ..Default::default()
        };
        let rules = vec![rule(1, 0, conditions, RuleState::Active)];

        let matching = RuleMatchInput {
            direction: Some("SPENDING"),
            ..input(Some("lidl"))
        };
        assert!(most_specific_match(&rules, &matching).is_some());

        let wrong_direction = RuleMatchInput {
            direction: Some("INCOME"),
            ..input(Some("lidl"))
        };
        assert!(most_specific_match(&rules, &wrong_direction).is_none());

        let wrong_key = RuleMatchInput {
            direction: Some("SPENDING"),
            ..input(Some("rewe"))
        };
        assert!(most_specific_match(&rules, &wrong_key).is_none());
    }

    #[test]
    fn only_active_rules_are_considered() {
        let conditions = RuleConditions {
            counterparty_key: Some("lidl".to_string()),
            ..Default::default()
        };
        let rules = vec![
            rule(1, 0, conditions.clone(), RuleState::InReview),
            rule(2, 0, conditions.clone(), RuleState::Revoked),
            rule(3, 0, conditions, RuleState::Rejected),
        ];
        assert!(most_specific_match(&rules, &input(Some("lidl"))).is_none());
    }

    #[test]
    fn higher_priority_wins_regardless_of_specificity() {
        let specific = RuleConditions {
            counterparty_key: Some("lidl".to_string()),
            counterparty_iban: Some("DE1".to_string()),
            ..Default::default()
        };
        let broad = RuleConditions {
            counterparty_key: Some("lidl".to_string()),
            ..Default::default()
        };
        let rules = vec![
            rule(1, 0, specific, RuleState::Active),
            rule(2, 10, broad, RuleState::Active),
        ];
        let winner = most_specific_match(&rules, &input(Some("lidl"))).unwrap();
        assert_eq!(winner.id, Uuid::from_bytes([2; 16]));
    }

    #[test]
    fn more_specific_rule_wins_at_equal_priority() {
        let narrow = RuleConditions {
            counterparty_key: Some("lidl".to_string()),
            counterparty_iban: Some("DE1".to_string()),
            ..Default::default()
        };
        let broad = RuleConditions {
            counterparty_key: Some("lidl".to_string()),
            ..Default::default()
        };
        let rules = vec![
            rule(1, 0, broad, RuleState::Active),
            rule(2, 0, narrow, RuleState::Active),
        ];
        let mut matching_input = input(Some("lidl"));
        matching_input.counterparty_iban = Some("DE1");
        let winner = most_specific_match(&rules, &matching_input).unwrap();
        assert_eq!(winner.id, Uuid::from_bytes([2; 16]));
    }

    #[test]
    fn bounded_amount_range_scores_as_one_not_two() {
        // A rule with only an upper amount bound (score contribution 1) vs
        // one with a bounded min+max *and* nothing else extra (also 1) tie
        // on specificity if both have the same other conditions; this test
        // instead proves a bounded range does not outscore a single extra
        // condition of its own weight (e.g. description_contains, weight 1).
        let bounded_range = RuleConditions {
            counterparty_key: Some("lidl".to_string()),
            amount_min: Some(Decimal::from(0)),
            amount_max: Some(Decimal::from(50)),
            ..Default::default()
        };
        let other_condition = RuleConditions {
            counterparty_key: Some("lidl".to_string()),
            description_contains: Some("foo".to_string()),
            ..Default::default()
        };
        assert_eq!(specificity(&bounded_range), specificity(&other_condition));
    }

    #[test]
    fn deterministic_tie_break_is_id_ascending() {
        let conditions = RuleConditions {
            counterparty_key: Some("lidl".to_string()),
            ..Default::default()
        };
        let rules = vec![
            rule(9, 0, conditions.clone(), RuleState::Active),
            rule(1, 0, conditions.clone(), RuleState::Active),
            rule(5, 0, conditions, RuleState::Active),
        ];
        let winner = most_specific_match(&rules, &input(Some("lidl"))).unwrap();
        assert_eq!(winner.id, Uuid::from_bytes([1; 16]));
    }

    #[test]
    fn invalid_regex_is_skipped_not_panicking() {
        let conditions = RuleConditions {
            description_regex: Some("(unclosed".to_string()),
            ..Default::default()
        };
        let rules = vec![rule(1, 0, conditions, RuleState::Active)];
        let mut matching_input = input(None);
        matching_input.description = Some("anything");
        // Must not panic, and an invalid regex never matches.
        assert!(most_specific_match(&rules, &matching_input).is_none());
    }

    #[test]
    fn valid_description_regex_matches() {
        let conditions = RuleConditions {
            description_regex: Some("^rent.*".to_string()),
            ..Default::default()
        };
        let rules = vec![rule(1, 0, conditions, RuleState::Active)];
        let mut matching = input(None);
        matching.description = Some("rent september");
        assert!(most_specific_match(&rules, &matching).is_some());

        let mut not_matching = input(None);
        not_matching.description = Some("groceries");
        assert!(most_specific_match(&rules, &not_matching).is_none());
    }

    #[test]
    fn description_contains_is_case_insensitive() {
        let conditions = RuleConditions {
            description_contains: Some("Rent".to_string()),
            ..Default::default()
        };
        let rules = vec![rule(1, 0, conditions, RuleState::Active)];
        let mut matching = input(None);
        matching.description = Some("september RENT payment");
        assert!(most_specific_match(&rules, &matching).is_some());
    }

    #[test]
    fn amount_range_is_inclusive_on_both_bounds() {
        let conditions = RuleConditions {
            amount_min: Some(Decimal::from(-50)),
            amount_max: Some(Decimal::from(-10)),
            ..Default::default()
        };
        let rules = vec![rule(1, 0, conditions, RuleState::Active)];

        let mut at_min = input(None);
        at_min.amount = Decimal::from(-50);
        assert!(most_specific_match(&rules, &at_min).is_some());

        let mut at_max = input(None);
        at_max.amount = Decimal::from(-10);
        assert!(most_specific_match(&rules, &at_max).is_some());

        let mut below_min = input(None);
        below_min.amount = Decimal::from(-51);
        assert!(most_specific_match(&rules, &below_min).is_none());

        let mut above_max = input(None);
        above_max.amount = Decimal::from(-9);
        assert!(most_specific_match(&rules, &above_max).is_none());
    }

    #[test]
    fn account_ids_condition_matches_membership() {
        let account_a = Uuid::from_bytes([1; 16]);
        let account_b = Uuid::from_bytes([2; 16]);
        let conditions = RuleConditions {
            account_ids: Some(vec![account_a]),
            ..Default::default()
        };
        let rules = vec![rule(1, 0, conditions, RuleState::Active)];

        let mut matching = input(None);
        matching.account_id = Some(account_a);
        assert!(most_specific_match(&rules, &matching).is_some());

        let mut not_matching = input(None);
        not_matching.account_id = Some(account_b);
        assert!(most_specific_match(&rules, &not_matching).is_none());
    }

    #[test]
    fn no_conditions_present_matches_everything_active() {
        let rules = vec![rule(1, 0, RuleConditions::default(), RuleState::Active)];
        assert!(most_specific_match(&rules, &input(None)).is_some());
    }

    fn corpus() -> Vec<RuleMatchInput<'static>> {
        let account_a = Uuid::from_bytes([0xaa; 16]);
        let account_b = Uuid::from_bytes([0xbb; 16]);
        vec![
            RuleMatchInput {
                counterparty_key: Some("lidl"),
                counterparty_iban: Some("DE01"),
                description: Some("LIDL SAGT DANKE 1234"),
                transaction_type: None,
                direction: Some("SPENDING"),
                amount: Decimal::from(-25),
                account_id: Some(account_a),
            },
            RuleMatchInput {
                counterparty_key: Some("lidl"),
                counterparty_iban: None,
                description: Some("Lidl refund"),
                transaction_type: None,
                direction: Some("INCOME"),
                amount: Decimal::from(5),
                account_id: Some(account_b),
            },
            RuleMatchInput {
                counterparty_key: Some("rewe"),
                counterparty_iban: None,
                description: None,
                transaction_type: None,
                direction: Some("SPENDING"),
                amount: Decimal::from(-60),
                account_id: Some(account_a),
            },
            RuleMatchInput {
                counterparty_key: None,
                counterparty_iban: None,
                description: Some("Salary"),
                transaction_type: None,
                direction: Some("INCOME"),
                amount: Decimal::from(2000),
                account_id: Some(account_b),
            },
        ]
    }

    #[test]
    fn count_matching_agrees_with_the_labelers_per_transaction_verdict() {
        let account_a = Uuid::from_bytes([0xaa; 16]);
        let cases = vec![
            // (conditions, expected reach over `corpus()`)
            (RuleConditions::default(), 4),
            (
                RuleConditions {
                    counterparty_key: Some("lidl".into()),
                    ..Default::default()
                },
                2,
            ),
            (
                RuleConditions {
                    counterparty_key: Some("lidl".into()),
                    direction: Some("SPENDING".into()),
                    ..Default::default()
                },
                1,
            ),
            (
                RuleConditions {
                    description_contains: Some("LIDL".into()),
                    ..Default::default()
                },
                2,
            ),
            (
                RuleConditions {
                    description_regex: Some(r"^lidl\b".into()),
                    ..Default::default()
                },
                0,
            ),
            (
                RuleConditions {
                    description_regex: Some(r"LIDL SAGT DANKE \d+".into()),
                    ..Default::default()
                },
                1,
            ),
            (
                RuleConditions {
                    amount_min: Some(Decimal::from(-30)),
                    amount_max: Some(Decimal::from(10)),
                    ..Default::default()
                },
                2,
            ),
            (
                RuleConditions {
                    account_ids: Some(vec![account_a]),
                    ..Default::default()
                },
                2,
            ),
            (
                RuleConditions {
                    counterparty_iban: Some("DE01".into()),
                    ..Default::default()
                },
                1,
            ),
            // Does not compile: matches nothing, never panics.
            (
                RuleConditions {
                    description_regex: Some("(".into()),
                    ..Default::default()
                },
                0,
            ),
        ];
        let inputs = corpus();
        for (conditions, expected) in cases {
            let count = count_matching(&conditions, &inputs);
            // The labeler's own verdict, transaction by transaction.
            let rules = vec![rule(1, 0, conditions.clone(), RuleState::Active)];
            let verdict = inputs
                .iter()
                .filter(|i| most_specific_match(&rules, i).is_some())
                .count();
            assert_eq!(count, verdict, "{conditions:?}");
            assert_eq!(count, expected, "{conditions:?}");
        }
    }

    #[test]
    fn count_matching_ignores_rule_state() {
        // Reach is independent of whether the rule is active: a revoked rule
        // still has a reach even though the labeler would skip it.
        let conditions = RuleConditions {
            counterparty_key: Some("lidl".into()),
            ..Default::default()
        };
        let revoked = vec![rule(1, 0, conditions.clone(), RuleState::Revoked)];
        let inputs = corpus();
        assert!(inputs.iter().all(|i| most_specific_match(&revoked, i).is_none()));
        assert_eq!(count_matching(&conditions, &inputs), 2);
    }
}
