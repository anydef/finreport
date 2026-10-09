//! §2.8 rule learning: promote a repeated `counterparty_key` → `category`
//! observation into a learned `rule` record once it crosses
//! `APP_rule_learn_min_observations` (default 3) with high enough confidence,
//! without ever overwriting a user-touched rule.
//!
//! WP0 stub — owned by WP2. Pure by design: `APP_rule_learn_min_observations`
//! and `APP_rule_auto_approve_threshold` (`utils::settings::Settings`) are
//! passed in as parameters rather than read here, so this module needs no
//! I/O to unit-test.

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::Decimal;

use crate::kafka::labeling::{RuleConditions, RuleOrigin, RuleRecord, RuleState};
use crate::labeling::normalize::normalize;
use crate::labeling::rules::{conditions_match, RuleMatchInput};

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
    /// The transaction's raw description - the evidence a description
    /// qualifier is derived from by [`consider_narrow`]. `None` when the
    /// transaction has none.
    pub description: Option<&'a str>,
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
    /// `Some` for a narrow rule from [`consider_narrow`]: a substring every
    /// agreeing description contains and no disagreeing one does. `None` for
    /// the ordinary merchant-wide rule.
    pub description_contains: Option<String>,
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
        description_contains: None,
    })
}

/// Every rule the learner should publish for one merchant's `observations`:
/// the merchant-wide rule when [`consider`] finds the merchant unambiguous
/// (one broad rule beats many narrow ones), otherwise the description-
/// qualified rules of [`consider_narrow`].
pub fn candidates(
    observations: &[Observation<'_>],
    min_observations: u32,
    min_user_observations: u32,
    auto_approve_threshold: f32,
) -> Vec<LearnedRule> {
    match consider(observations, min_observations, min_user_observations, auto_approve_threshold) {
        Some(broad) => vec![broad],
        None => consider_narrow(observations, min_observations, min_user_observations, auto_approve_threshold),
    }
}

/// Shortest description token a narrow rule may be keyed on. Below this a
/// token is an abbreviation or fragment ("pp", "b.v" -> "b", "sa") that
/// matches far more than the one subscription it was seen on.
pub const MIN_NEEDLE_LEN: usize = 4;

/// Generic payment-boilerplate words that appear on many unrelated
/// descriptions of one processor (PayPal's "Ihr Einkauf bei ..."), so they say
/// nothing about *which* purchase it was. Verification would usually reject
/// them anyway; excluding them up front stops a one-decision-per-category
/// group from keying on them just because the other group's wording differs.
const GENERIC_TOKENS: &[&str] = &[
    "einkauf", "zahlung", "zahlungen", "payment", "lastschrift", "gutschrift", "rechnung",
    "bestellung", "order", "buchung", "abbuchung", "referenz", "kundenreferenz", "mandat",
    "mandatsreferenz", "danke", "vielen", "purchase", "invoice", "europe", "luxembourg",
    "sarl", "ihre", "ihren", "ihrem",
];

/// The fallback of [`consider`] for a merchant whose categories disagree
/// (PayPal, Amazon): instead of giving up, learn one *description-qualified*
/// rule per category that has enough agreeing evidence of its own.
///
/// Call it only after [`consider`] returned `None`; a merchant-wide rule
/// always wins when the merchant is unambiguous, so this returns an empty
/// vec unless the evidence names at least two categories.
///
/// Per category, the same evidence thresholds as [`consider`] apply (it is
/// run over that category's observations alone, which also yields the
/// confidence and auto-approval). The qualifier is then derived by
/// [`derive_needle`] and **verified** against every other-category
/// observation; a category whose qualifier cannot be verified learns
/// nothing. As in [`consider`], human decisions dominate: with any
/// `user_confirmed` observation present, only those are evidence.
///
/// Output is ordered by category slug, so it is deterministic.
pub fn consider_narrow(
    observations: &[Observation<'_>],
    min_observations: u32,
    min_user_observations: u32,
    auto_approve_threshold: f32,
) -> Vec<LearnedRule> {
    let Some(first) = observations.first() else {
        return Vec::new();
    };
    let counterparty_key = first.counterparty_key;

    let any_user = observations.iter().any(|o| o.user_confirmed);
    let evidence: Vec<&Observation<'_>> =
        observations.iter().filter(|o| !any_user || o.user_confirmed).collect();

    let mut by_category: BTreeMap<&str, Vec<&Observation<'_>>> = BTreeMap::new();
    for o in &evidence {
        by_category.entry(o.category_slug).or_default().push(o);
    }
    if by_category.len() < 2 {
        return Vec::new();
    }

    let mut rules = Vec::new();
    for (category, group) in &by_category {
        let owned: Vec<Observation<'_>> = group.iter().map(|o| (*o).clone()).collect();
        let Some(base) = consider(&owned, min_observations, min_user_observations, auto_approve_threshold) else {
            continue;
        };
        let others: Vec<&Observation<'_>> =
            evidence.iter().filter(|o| o.category_slug != *category).copied().collect();
        let Some(needle) = derive_needle(counterparty_key, group, &others) else {
            continue;
        };
        rules.push(LearnedRule { description_contains: Some(needle), ..base });
    }
    rules
}

/// Derives the `description_contains` for `group` (all one category) so that
/// it separates them from `others` (every observation of another category).
///
/// Candidates are single tokens of the *normalised* description (the same
/// [`normalize`] that builds `counterparty_key`: lowercased, accents folded,
/// card-terminal noise and a trailing date/store number dropped), split on
/// non-alphanumerics, that
/// - appear in **every** description of the group,
/// - are alphabetic only - so no reference number, date, or id fragment can
///   ever be chosen - and at least [`MIN_NEEDLE_LEN`] long,
/// - are not generic payment boilerplate and not part of the merchant key
///   (which the rule already requires).
///
/// Candidates are ranked by how early they sit in the descriptions (a
/// merchant name leads, trailing wording is noise), then longer, then
/// alphabetically, so the choice is deterministic. The first candidate that
/// passes [`needle_separates`] wins; if none does, `None`.
pub fn derive_needle(
    counterparty_key: &str,
    group: &[&Observation<'_>],
    others: &[&Observation<'_>],
) -> Option<String> {
    let token_lists: Vec<Vec<String>> = group
        .iter()
        .map(|o| o.description.map(tokens).unwrap_or_default())
        .collect();
    let first = token_lists.first()?;
    if token_lists.iter().any(Vec::is_empty) {
        return None;
    }
    let key_tokens: BTreeSet<String> = tokens(counterparty_key).into_iter().collect();

    // (summed position, token) for each token common to every description.
    let mut candidates: Vec<(usize, &String)> = Vec::new();
    for token in first.iter().collect::<BTreeSet<_>>() {
        if !is_distinctive(token, &key_tokens) {
            continue;
        }
        let positions: Option<Vec<usize>> =
            token_lists.iter().map(|list| list.iter().position(|t| t == token)).collect();
        if let Some(positions) = positions {
            candidates.push((positions.iter().sum(), token));
        }
    }
    candidates.sort_by(|(pos_a, a), (pos_b, b)| {
        pos_a.cmp(pos_b).then_with(|| b.len().cmp(&a.len())).then_with(|| a.cmp(b))
    });

    candidates
        .into_iter()
        .map(|(_, token)| token)
        .find(|token| needle_separates(counterparty_key, token, group, others))
        .cloned()
}

/// The explicit pre-publication check: re-runs the real rule matcher
/// ([`conditions_match`], the very function the labeler resolves with) for
/// the candidate rule `counterparty_key AND description_contains = needle`
/// over the raw descriptions, and requires that it matches **every**
/// `group` observation and **none** of `others`.
pub fn needle_separates(
    counterparty_key: &str,
    needle: &str,
    group: &[&Observation<'_>],
    others: &[&Observation<'_>],
) -> bool {
    let conditions = RuleConditions {
        counterparty_key: Some(counterparty_key.to_string()),
        description_contains: Some(needle.to_string()),
        ..Default::default()
    };
    let matches = |o: &Observation<'_>| {
        conditions_match(
            &conditions,
            &RuleMatchInput {
                counterparty_key: Some(o.counterparty_key),
                counterparty_iban: None,
                description: o.description,
                transaction_type: None,
                direction: None,
                amount: Decimal::ZERO,
                account_id: None,
            },
        )
    };
    group.iter().all(|o| matches(o)) && !others.iter().any(|o| matches(o))
}

/// Tokens of the normalised text, in order.
fn tokens(text: &str) -> Vec<String> {
    normalize(None, Some(text))
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn is_distinctive(token: &str, key_tokens: &BTreeSet<String>) -> bool {
    token.chars().count() >= MIN_NEEDLE_LEN
        && token.chars().all(char::is_alphabetic)
        && !GENERIC_TOKENS.contains(&token)
        && !key_tokens.contains(token)
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
            description: None,
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
            description_contains: None,
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

    // ---- description-qualified (narrow) rules ----

    fn paypal<'a>(category_slug: &'a str, description: &'a str, user: bool) -> Observation<'a> {
        Observation {
            counterparty_key: "paypal",
            category_slug,
            confidence: 1.0,
            user_confirmed: user,
            description: Some(description),
        }
    }

    const NETFLIX: &str = "PayPal . NETFLIX INTERNATIONAL B.V. 1234567890 PP.1234.PP . , Ihr Einkauf bei NETFLIX";
    const SPOTIFY: &str = "PayPal . SPOTIFY AB 9876543210 PP.9876.PP . , Ihr Einkauf bei SPOTIFY";
    const SHOP: &str = "PayPal . ETSY IRELAND UC 5550001112 PP.5550.PP . , Ihr Einkauf bei ETSY";

    fn narrow(observations: &[Observation<'_>]) -> Vec<LearnedRule> {
        candidates(observations, MIN_OBSERVATIONS, MIN_USER_OBSERVATIONS, THRESHOLD)
    }

    fn needles(rules: &[LearnedRule]) -> Vec<(String, Option<String>)> {
        rules.iter().map(|r| (r.category_slug.clone(), r.description_contains.clone())).collect()
    }

    #[test]
    fn two_paypal_categories_learn_two_narrow_rules_and_no_broad_one() {
        let rules = narrow(&[
            paypal("media.streaming", NETFLIX, true),
            paypal("shopping.online", SHOP, true),
        ]);
        assert_eq!(
            needles(&rules),
            vec![
                ("media.streaming".to_string(), Some("netflix".to_string())),
                ("shopping.online".to_string(), Some("etsy".to_string())),
            ]
        );
        assert!(rules.iter().all(|r| r.description_contains.is_some()), "no merchant-wide rule");
        assert!(rules.iter().all(|r| r.auto_approved && r.confidence == 1.0));
    }

    #[test]
    fn an_unambiguous_merchant_learns_one_broad_rule_not_a_narrow_one() {
        let rules = narrow(&[
            paypal("media.streaming", NETFLIX, true),
            paypal("media.streaming", SPOTIFY, true),
        ]);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].description_contains, None);
        assert_eq!(rules[0].category_slug, "media.streaming");
    }

    #[test]
    fn narrow_needs_the_same_evidence_as_a_broad_rule() {
        // Two user decisions per category required: one each is not enough.
        let observations = [paypal("a", NETFLIX, true), paypal("b", SHOP, true)];
        assert!(consider_narrow(&observations, MIN_OBSERVATIONS, 2, THRESHOLD).is_empty());
        let observations = [
            paypal("a", NETFLIX, true),
            paypal("a", NETFLIX, true),
            paypal("b", SHOP, true),
        ];
        let rules = consider_narrow(&observations, MIN_OBSERVATIONS, 2, THRESHOLD);
        assert_eq!(needles(&rules), vec![("a".to_string(), Some("netflix".to_string()))]);
    }

    #[test]
    fn descriptions_sharing_nothing_distinctive_learn_nothing() {
        // Nothing distinctive: the key, a 2-letter fragment, digits.
        let rules = narrow(&[
            paypal("a", "PayPal . AB 12345 PP", true),
            paypal("b", "PayPal . CD 67890 QQ", true),
        ]);
        assert!(rules.is_empty(), "got {rules:?}");
    }

    #[test]
    fn a_missing_description_learns_nothing() {
        let mut no_description = paypal("a", "x", true);
        no_description.description = None;
        assert!(narrow(&[no_description, paypal("b", SHOP, true)])
            .iter()
            .all(|r| r.category_slug != "a"));
    }

    #[test]
    fn a_needle_that_also_matches_a_disagreeing_transaction_is_rejected() {
        let a = paypal("a", NETFLIX, true);
        let b = paypal("b", "PayPal . NETFLIX GIFTCARD 777 VIA EBAY", true);
        // "netflix" is in both groups: verification must refuse it ...
        assert!(!needle_separates("paypal", "netflix", &[&a], &[&b]));
        // ... a token only the group has is accepted ...
        assert!(needle_separates("paypal", "international", &[&a], &[&b]));
        // ... and derivation walks past the rejected token to that one.
        assert_eq!(derive_needle("paypal", &[&a], &[&b]), Some("international".to_string()));
        // With nothing left to separate on, nothing is learned.
        let b2 = paypal("b", NETFLIX, true);
        assert_eq!(derive_needle("paypal", &[&a], &[&b2]), None);
    }

    #[test]
    fn verification_runs_on_raw_descriptions_not_the_normalised_ones() {
        // Normalisation folds the umlaut ("muller"); the raw text keeps it, so
        // that token would never match in production. Verification against the
        // raw description catches it.
        let a = paypal("a", "PayPal . MÜLLER DROGERIE 12", true);
        let b = paypal("b", SHOP, true);
        assert!(!needle_separates("paypal", "muller", &[&a], &[&b]));
        assert_eq!(derive_needle("paypal", &[&a], &[&b]), Some("drogerie".to_string()));
    }

    #[test]
    fn digits_references_and_dates_are_never_the_distinguishing_token() {
        let a = paypal("a", "PayPal 1234567890 15.03.2026 REF4711 PP.1234.PP", true);
        let b = paypal("b", "PayPal 9999999999 16.03.2026 REF0815 PP.9999.PP", true);
        assert_eq!(derive_needle("paypal", &[&a], &[&b]), None);
        for token in ["1234567890", "ref4711", "15", "pp"] {
            assert!(!is_distinctive(token, &BTreeSet::new()), "{token}");
        }
    }

    #[test]
    fn derivation_is_deterministic_and_prefers_the_leading_merchant_name() {
        let a = paypal("a", "PayPal . NETFLIX INTERNATIONAL B.V.", true);
        let b = paypal("b", SHOP, true);
        let first = derive_needle("paypal", &[&a], &[&b]);
        assert_eq!(first, Some("netflix".to_string()));
        for _ in 0..5 {
            assert_eq!(derive_needle("paypal", &[&a], &[&b]), first);
        }
    }

    #[test]
    fn a_shared_token_across_a_multi_observation_group_is_found() {
        let a1 = paypal("a", "PayPal . NETFLIX INTERNATIONAL 111", true);
        let a2 = paypal("a", "PayPal . NETFLIX INTERNATIONAL 222 Rechnung", true);
        let b = paypal("b", SHOP, true);
        let rules = consider_narrow(&[a1, a2, b], MIN_OBSERVATIONS, 2, THRESHOLD);
        assert_eq!(needles(&rules), vec![("a".to_string(), Some("netflix".to_string()))]);
    }

    #[test]
    fn narrow_rules_do_not_match_each_others_evidence() {
        let observations =
            [paypal("a", NETFLIX, true), paypal("b", SPOTIFY, true), paypal("c", SHOP, true)];
        let rules = narrow(&observations);
        assert_eq!(rules.len(), 3);
        for rule in &rules {
            let needle = rule.description_contains.as_deref().unwrap();
            for o in &observations {
                let hit = o.description.unwrap().to_lowercase().contains(needle);
                assert_eq!(hit, o.category_slug == rule.category_slug, "{needle} vs {}", o.category_slug);
            }
        }
    }

    #[test]
    fn llm_guesses_do_not_veto_a_human_narrow_rule() {
        let mut guess = paypal("b", NETFLIX, false);
        guess.confidence = 0.95;
        let rules = narrow(&[paypal("a", NETFLIX, true), paypal("b", SHOP, true), guess]);
        // Human evidence only: the LLM's NETFLIX->b guess is the very thing
        // the user overruled, so "netflix" still verifies for "a".
        assert!(needles(&rules).contains(&("a".to_string(), Some("netflix".to_string()))));
    }

    #[test]
    fn a_narrow_rule_outranks_the_broad_one_in_most_specific_match() {
        use crate::labeling::rules::most_specific_match;
        let record = |id: u128, contains: Option<&str>, category: &str| {
            let mut r = rule_record(RuleOrigin::Learned, RuleState::Active, false, category, Some(1.0));
            r.id = Uuid::from_u128(id);
            r.conditions = RuleConditions {
                counterparty_key: Some("paypal".into()),
                description_contains: contains.map(str::to_string),
                ..Default::default()
            };
            r
        };
        // The broad rule gets the *smaller* id, which wins an exact tie, so
        // only genuine specificity can put the narrow rule first.
        let rules = vec![record(1, None, "broad"), record(2, Some("netflix"), "narrow")];
        let input = |d: &'static str| RuleMatchInput {
            counterparty_key: Some("paypal"),
            counterparty_iban: None,
            description: Some(d),
            transaction_type: None,
            direction: None,
            amount: Decimal::ZERO,
            account_id: None,
        };
        assert_eq!(most_specific_match(&rules, &input(NETFLIX)).unwrap().category_slug, "narrow");
        assert_eq!(most_specific_match(&rules, &input(SHOP)).unwrap().category_slug, "broad");
    }
}
