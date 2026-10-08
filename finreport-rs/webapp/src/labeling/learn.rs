//! §2.8 rule learning: promote a repeated `counterparty_key` → `category`
//! observation into a learned `rule` record once it crosses
//! `APP_rule_learn_min_observations` (default 3) with high enough confidence,
//! without ever overwriting a user-touched rule.
//!
//! WP0 stub — owned by WP2. Pure by design: `APP_rule_learn_min_observations`
//! and `APP_rule_auto_approve_threshold` (`utils::settings::Settings`) are
//! passed in as parameters rather than read here, so this module needs no
//! I/O to unit-test.

use crate::kafka::labeling::{RuleOrigin, RuleRecord, RuleState};

/// One observed (`counterparty_key`, resolved category) pair the learner is
/// considering promoting to a rule.
#[derive(Debug, Clone)]
pub struct Observation<'a> {
    pub counterparty_key: &'a str,
    pub category_slug: &'a str,
    /// `1.0` for a user-confirmed label (§2.8), the LLM's own confidence
    /// otherwise.
    pub confidence: f32,
    /// `true` for a human decision (`label_source = 'user'`).
    pub user_confirmed: bool,
}

/// A candidate learned rule, not yet published.
#[derive(Debug, Clone, PartialEq)]
pub struct LearnedRule {
    pub counterparty_key: String,
    pub category_slug: String,
    pub confidence: f32,
    /// `true` at/above `APP_rule_auto_approve_threshold` (default `0.9`,
    /// inclusive — the exact boundary §8 calls out).
    pub auto_approved: bool,
}

/// Decides whether `observations` (all sharing one `counterparty_key`) should
/// become a learned rule, and whether it auto-approves.
///
/// - **Human evidence dominates.** If any observation is `user_confirmed`:
///   user decisions naming different categories => `None` (a conflicting
///   human history is not a rule, and is never written as `in_review`
///   either); otherwise the user's category is the candidate, and LLM
///   observations naming some *other* category are discarded as the guesses
///   the human just overruled (counting them as conflict would let a wrong
///   LLM answer veto the very correction that rejects it). It qualifies at
///   `min_user_observations` user decisions, with confidence taken from the
///   user observations only (1.0), or failing that via the LLM threshold
///   over every agreeing observation.
/// - **LLM-only evidence**: fewer than `min_observations` => `None`; any two
///   observations naming different `category_slug`s => `None`; confidence is
///   the **minimum** over all observations (never an average).
/// - `auto_approved` is set at `confidence >= auto_approve_threshold`,
///   inclusive at the boundary.
pub fn consider(
    observations: &[Observation<'_>],
    min_observations: u32,
    min_user_observations: u32,
    auto_approve_threshold: f32,
) -> Option<LearnedRule> {
    let first = observations.first()?;
    let counterparty_key = first.counterparty_key;

    let user: Vec<&Observation<'_>> = observations.iter().filter(|o| o.user_confirmed).collect();
    let (category_slug, evidence): (&str, Vec<&Observation<'_>>) = if let Some(first_user) = user.first() {
        let category_slug = first_user.category_slug;
        if user.iter().any(|o| o.category_slug != category_slug) {
            return None;
        }
        let agreeing: Vec<&Observation<'_>> = observations
            .iter()
            .filter(|o| o.category_slug == category_slug)
            .collect();
        if (user.len() as u32) >= min_user_observations {
            (category_slug, user)
        } else if (agreeing.len() as u32) >= min_observations {
            (category_slug, agreeing)
        } else {
            return None;
        }
    } else {
        if (observations.len() as u32) < min_observations {
            return None;
        }
        let category_slug = first.category_slug;
        if observations.iter().any(|o| o.category_slug != category_slug) {
            return None;
        }
        (category_slug, observations.iter().collect())
    };

    let confidence = evidence
        .iter()
        .map(|o| o.confidence)
        .fold(f32::INFINITY, f32::min);

    Some(LearnedRule {
        counterparty_key: counterparty_key.to_string(),
        category_slug: category_slug.to_string(),
        confidence,
        auto_approved: confidence >= auto_approve_threshold,
    })
}

/// Whether the learner may publish `candidate` given whatever rule record
/// (if any) already occupies its deterministic id
/// (`learned_rule_uuid(counterparty_key, category_slug)`).
///
/// **The learner never overwrites a human.** It publishes only when
/// `existing` is `None` (no rule exists yet for this id — which also covers
/// a `counterparty_key` whose only rule is for a *different* category: that
/// rule lives at a different id and never comes through `existing` here), or
/// when `existing` is `origin = learned`, `state = in_review` and
/// `user_touched = false`. Any rule the user approved, edited, rejected or
/// revoked — and that therefore fails that test — is left exactly as it is;
/// new evidence changes nothing. The same test is why a **revoked** rule is
/// never re-learned: its `state` is `Revoked`, not `InReview`.
///
/// Even when eligible, republishing an unchanged candidate (same category,
/// same confidence) is a no-op — idempotency, not a repeated event for
/// identical evidence.
pub fn should_publish(existing: Option<&RuleRecord>, candidate: &LearnedRule) -> bool {
    match existing {
        None => true,
        Some(rule) => {
            let eligible = rule.origin == RuleOrigin::Learned
                && rule.state == RuleState::InReview
                && !rule.user_touched;
            if !eligible {
                return false;
            }
            let unchanged = rule.category_slug == candidate.category_slug
                && rule.confidence == Some(candidate.confidence);
            !unchanged
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    const MIN_OBSERVATIONS: u32 = 3;
    const MIN_USER_OBSERVATIONS: u32 = 1;
    const THRESHOLD: f32 = 0.9;

    fn obs<'a>(category_slug: &'a str, confidence: f32) -> Observation<'a> {
        Observation {
            counterparty_key: "lidl",
            category_slug,
            confidence,
            user_confirmed: false,
        }
    }

    fn user_obs<'a>(category_slug: &'a str) -> Observation<'a> {
        Observation { user_confirmed: true, ..obs(category_slug, 1.0) }
    }

    #[test]
    fn a_single_user_decision_is_a_candidate_and_auto_approves() {
        let candidate = consider(&[user_obs("food.groceries")], MIN_OBSERVATIONS, MIN_USER_OBSERVATIONS, THRESHOLD).unwrap();
        assert_eq!(candidate.category_slug, "food.groceries");
        assert_eq!(candidate.confidence, 1.0);
        assert!(candidate.auto_approved);
    }

    #[test]
    fn a_single_llm_observation_is_still_not_a_candidate() {
        assert_eq!(consider(&[obs("food.groceries", 1.0)], MIN_OBSERVATIONS, MIN_USER_OBSERVATIONS, THRESHOLD), None);
    }

    #[test]
    fn conflicting_user_decisions_disqualify_even_when_each_alone_would_qualify() {
        let observations = vec![user_obs("food.groceries"), user_obs("food.restaurants")];
        assert_eq!(consider(&observations, MIN_OBSERVATIONS, MIN_USER_OBSERVATIONS, THRESHOLD), None);
    }

    #[test]
    fn a_user_decision_overrules_disagreeing_llm_guesses() {
        let observations = vec![
            obs("food.restaurants", 0.95),
            obs("food.restaurants", 0.95),
            user_obs("food.groceries"),
        ];
        let candidate = consider(&observations, MIN_OBSERVATIONS, MIN_USER_OBSERVATIONS, THRESHOLD).unwrap();
        assert_eq!(candidate.category_slug, "food.groceries");
        assert_eq!(candidate.confidence, 1.0);
    }

    #[test]
    fn a_higher_user_threshold_needs_more_user_decisions_or_llm_agreement() {
        let one_user = vec![user_obs("food.groceries")];
        assert_eq!(consider(&one_user, MIN_OBSERVATIONS, 2, THRESHOLD), None);
        let two_users = vec![user_obs("food.groceries"), user_obs("food.groceries")];
        assert!(consider(&two_users, MIN_OBSERVATIONS, 2, THRESHOLD).is_some());
        // One user + two agreeing LLM guesses reaches the LLM threshold of 3,
        // at the weakest confidence among them.
        let mixed = vec![user_obs("food.groceries"), obs("food.groceries", 0.8), obs("food.groceries", 0.95)];
        let candidate = consider(&mixed, MIN_OBSERVATIONS, 2, THRESHOLD).unwrap();
        assert_eq!(candidate.confidence, 0.8);
        assert!(!candidate.auto_approved);
    }

    #[test]
    fn fewer_than_min_observations_is_not_a_candidate() {
        let observations = vec![obs("food.groceries", 1.0), obs("food.groceries", 1.0)];
        assert_eq!(
            consider(&observations, MIN_OBSERVATIONS, MIN_USER_OBSERVATIONS, THRESHOLD),
            None,
            "N-1 observations must not yield a candidate"
        );
    }

    #[test]
    fn exactly_min_observations_is_a_candidate() {
        let observations = vec![
            obs("food.groceries", 1.0),
            obs("food.groceries", 1.0),
            obs("food.groceries", 1.0),
        ];
        let candidate = consider(&observations, MIN_OBSERVATIONS, MIN_USER_OBSERVATIONS, THRESHOLD).unwrap();
        assert_eq!(candidate.category_slug, "food.groceries");
        assert_eq!(candidate.confidence, 1.0);
        assert!(candidate.auto_approved);
    }

    #[test]
    fn any_conflicting_category_disqualifies_outright() {
        let observations = vec![
            obs("food.groceries", 1.0),
            obs("food.groceries", 1.0),
            obs("food.restaurants", 1.0),
            obs("food.groceries", 1.0),
        ];
        assert_eq!(consider(&observations, MIN_OBSERVATIONS, MIN_USER_OBSERVATIONS, THRESHOLD), None);
    }

    #[test]
    fn confidence_is_the_minimum_not_the_average() {
        let observations = vec![obs("food.groceries", 0.95), obs("food.groceries", 0.6), obs("food.groceries", 0.99)];
        let candidate = consider(&observations, MIN_OBSERVATIONS, MIN_USER_OBSERVATIONS, THRESHOLD).unwrap();
        assert_eq!(candidate.confidence, 0.6);
        assert!(!candidate.auto_approved);
    }

    #[test]
    fn user_confirmed_observations_count_as_full_confidence() {
        // Three user confirmations (confidence 1.0 each, per the caller's
        // convention) give confidence 1.0, auto-approved.
        let observations = vec![obs("food.groceries", 1.0), obs("food.groceries", 1.0), obs("food.groceries", 1.0)];
        let candidate = consider(&observations, MIN_OBSERVATIONS, MIN_USER_OBSERVATIONS, THRESHOLD).unwrap();
        assert_eq!(candidate.confidence, 1.0);
        assert!(candidate.auto_approved);
    }

    #[test]
    fn auto_approve_threshold_is_inclusive_at_exactly_0_9() {
        let observations = vec![obs("food.groceries", 0.9), obs("food.groceries", 0.95), obs("food.groceries", 0.99)];
        let candidate = consider(&observations, MIN_OBSERVATIONS, MIN_USER_OBSERVATIONS, THRESHOLD).unwrap();
        assert_eq!(candidate.confidence, 0.9);
        assert!(candidate.auto_approved, "0.9 must auto-approve, inclusive");
    }

    #[test]
    fn just_below_auto_approve_threshold_stays_in_review() {
        let observations = vec![obs("food.groceries", 0.899_999), obs("food.groceries", 1.0), obs("food.groceries", 1.0)];
        let candidate = consider(&observations, MIN_OBSERVATIONS, MIN_USER_OBSERVATIONS, THRESHOLD).unwrap();
        assert!(!candidate.auto_approved);
    }

    fn rule_record(origin: RuleOrigin, state: RuleState, user_touched: bool, category_slug: &str, confidence: Option<f32>) -> RuleRecord {
        RuleRecord {
            schema_version: 1,
            id: Uuid::new_v4(),
            name: "learned".to_string(),
            category_slug: category_slug.to_string(),
            conditions: Default::default(),
            priority: 0,
            state,
            origin,
            auto_approved: false,
            user_touched,
            confidence,
            evidence: None,
            created_at: Utc::now(),
            revision: Utc::now(),
        }
    }

    fn candidate(category_slug: &str, confidence: f32) -> LearnedRule {
        LearnedRule {
            counterparty_key: "lidl".to_string(),
            category_slug: category_slug.to_string(),
            confidence,
            auto_approved: confidence >= THRESHOLD,
        }
    }

    #[test]
    fn publishes_when_no_rule_exists_yet() {
        assert!(should_publish(None, &candidate("food.groceries", 1.0)));
    }

    #[test]
    fn publishes_when_existing_is_untouched_learned_in_review() {
        let existing = rule_record(RuleOrigin::Learned, RuleState::InReview, false, "food.groceries", Some(0.6));
        // Confidence changed (more evidence arrived) -> republish.
        assert!(should_publish(Some(&existing), &candidate("food.groceries", 0.95)));
    }

    #[test]
    fn idempotent_when_candidate_is_unchanged() {
        let existing = rule_record(RuleOrigin::Learned, RuleState::InReview, false, "food.groceries", Some(0.95));
        assert!(!should_publish(Some(&existing), &candidate("food.groceries", 0.95)));
    }

    #[test]
    fn never_overwrites_a_user_touched_rule() {
        let existing = rule_record(RuleOrigin::Learned, RuleState::InReview, true, "food.groceries", Some(0.6));
        assert!(!should_publish(Some(&existing), &candidate("food.groceries", 0.95)));
    }

    #[test]
    fn never_overwrites_an_active_rule() {
        let existing = rule_record(RuleOrigin::Learned, RuleState::Active, false, "food.groceries", Some(0.95));
        assert!(!should_publish(Some(&existing), &candidate("food.groceries", 0.6)));
    }

    #[test]
    fn never_overwrites_a_rejected_rule() {
        let existing = rule_record(RuleOrigin::Learned, RuleState::Rejected, false, "food.groceries", Some(0.6));
        assert!(!should_publish(Some(&existing), &candidate("food.groceries", 0.95)));
    }

    #[test]
    fn revoked_rule_is_never_re_learned() {
        let existing = rule_record(RuleOrigin::Learned, RuleState::Revoked, false, "food.groceries", Some(0.95));
        assert!(!should_publish(Some(&existing), &candidate("food.groceries", 1.0)));
    }

    #[test]
    fn never_overwrites_a_user_authored_rule_even_if_in_review() {
        let existing = rule_record(RuleOrigin::User, RuleState::InReview, false, "food.groceries", None);
        assert!(!should_publish(Some(&existing), &candidate("food.groceries", 1.0)));
    }
}
