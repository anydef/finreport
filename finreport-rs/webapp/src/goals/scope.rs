//! The scope predicate (iteration 4 §3.2): pure, no database.

use entity::entities::goal;

use crate::kafka::goals::Combine;

/// What a goal is measured against, parsed from the `goal` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub category_slugs: Vec<String>,
    pub tags: Vec<String>,
    /// How the category condition and the tag condition join.
    pub combine: Combine,
    /// How the listed tags join each other.
    pub tag_combine: Combine,
}

fn parse_combine(s: &str) -> Combine {
    if s == "any" {
        Combine::Any
    } else {
        Combine::All
    }
}

impl From<&goal::Model> for Scope {
    fn from(g: &goal::Model) -> Self {
        Scope {
            category_slugs: g.scope_category_slugs.clone(),
            tags: g.scope_tags.clone(),
            combine: parse_combine(&g.scope_combine),
            tag_combine: parse_combine(&g.scope_tag_combine),
        }
    }
}

/// True when `slug` equals `ancestor` or is a descendant of it. Slugs are
/// dotted materialized paths (`leisure.hobbies.crafts`), so a descendant is a
/// prefix match on a whole path segment (`leisure` does not cover `leisurely`).
pub fn is_same_or_descendant(slug: &str, ancestor: &str) -> bool {
    slug == ancestor
        || (slug.len() > ancestor.len()
            && slug.starts_with(ancestor)
            && slug.as_bytes()[ancestor.len()] == b'.')
}

impl Scope {
    pub fn has_categories(&self) -> bool {
        !self.category_slugs.is_empty()
    }

    pub fn has_tags(&self) -> bool {
        !self.tags.is_empty()
    }

    /// An empty scope matches nothing (it is rejected at mutation time; this
    /// is the belt to that braces).
    pub fn is_empty(&self) -> bool {
        !self.has_categories() && !self.has_tags()
    }

    fn category_condition(&self, category: Option<&str>) -> bool {
        match category {
            Some(slug) => self
                .category_slugs
                .iter()
                .any(|s| is_same_or_descendant(slug, s)),
            None => false,
        }
    }

    fn tag_condition(&self, row_tags: &[String]) -> bool {
        let has = |t: &String| row_tags.contains(t);
        match self.tag_combine {
            Combine::All => self.tags.iter().all(has),
            Combine::Any => self.tags.iter().any(has),
        }
    }

    fn join(&self, category_ok: bool, tags_ok: bool) -> bool {
        match (self.has_categories(), self.has_tags()) {
            (false, false) => false,
            (true, false) => category_ok,
            (false, true) => tags_ok,
            (true, true) => match self.combine {
                Combine::All => category_ok && tags_ok,
                Combine::Any => category_ok || tags_ok,
            },
        }
    }

    /// Does a row with this category and tags fall in scope?
    pub fn matches(&self, category: Option<&str>, row_tags: &[String]) -> bool {
        self.join(self.category_condition(category), self.tag_condition(row_tags))
    }

    /// Would the row be in scope if its (missing) category turned out to be
    /// one the scope names? Used to decide whether an uncategorised row is
    /// `pending` (§3.3).
    pub fn matches_if_category_resolved(&self, row_tags: &[String]) -> bool {
        self.has_categories() && self.join(true, self.tag_condition(row_tags))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(cats: &[&str], tags: &[&str], combine: Combine, tag_combine: Combine) -> Scope {
        Scope {
            category_slugs: cats.iter().map(|s| s.to_string()).collect(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            combine,
            tag_combine,
        }
    }

    fn tags(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn category_matches_itself_and_descendants_at_each_depth() {
        let s = scope(&["leisure"], &[], Combine::All, Combine::All);
        assert!(s.matches(Some("leisure"), &[]));
        assert!(s.matches(Some("leisure.hobbies"), &[]));
        assert!(s.matches(Some("leisure.hobbies.crafts"), &[]));
        let mid = scope(&["leisure.hobbies"], &[], Combine::All, Combine::All);
        assert!(mid.matches(Some("leisure.hobbies.crafts"), &[]));
        assert!(!mid.matches(Some("leisure"), &[]));
        assert!(!mid.matches(Some("leisure.sports"), &[]));
    }

    #[test]
    fn prefix_must_end_on_a_path_segment() {
        let s = scope(&["leisure"], &[], Combine::All, Combine::All);
        assert!(!s.matches(Some("leisurely"), &[]));
        assert!(!s.matches(Some("leisure_x.y"), &[]));
    }

    #[test]
    fn several_categories_combine_with_or() {
        let s = scope(&["food", "leisure.hobbies"], &[], Combine::All, Combine::All);
        assert!(s.matches(Some("food.groceries"), &[]));
        assert!(s.matches(Some("leisure.hobbies"), &[]));
        assert!(!s.matches(Some("transport"), &[]));
    }

    #[test]
    fn uncategorised_row_never_matches_a_category_condition() {
        let s = scope(&["food"], &[], Combine::All, Combine::All);
        assert!(!s.matches(None, &[]));
    }

    #[test]
    fn tag_combine_all_requires_every_tag() {
        let s = scope(&[], &["a", "b"], Combine::All, Combine::All);
        assert!(s.matches(None, &tags(&["a", "b", "c"])));
        assert!(!s.matches(None, &tags(&["a"])));
    }

    #[test]
    fn tag_combine_any_requires_one_tag() {
        let s = scope(&[], &["a", "b"], Combine::All, Combine::Any);
        assert!(s.matches(None, &tags(&["b"])));
        assert!(!s.matches(None, &tags(&["c"])));
        assert!(!s.matches(None, &[]));
    }

    #[test]
    fn combine_all_requires_both_conditions() {
        let s = scope(&["food"], &["trip"], Combine::All, Combine::All);
        assert!(s.matches(Some("food"), &tags(&["trip"])));
        assert!(!s.matches(Some("food"), &[]));
        assert!(!s.matches(Some("transport"), &tags(&["trip"])));
    }

    #[test]
    fn combine_any_requires_either_condition() {
        let s = scope(&["food"], &["trip"], Combine::Any, Combine::All);
        assert!(s.matches(Some("food"), &[]));
        assert!(s.matches(Some("transport"), &tags(&["trip"])));
        assert!(!s.matches(Some("transport"), &[]));
    }

    #[test]
    fn single_kind_scope_ignores_the_combine_flag() {
        for c in [Combine::All, Combine::Any] {
            let cats = scope(&["food"], &[], c, Combine::All);
            assert!(cats.matches(Some("food"), &[]));
            let only_tags = scope(&[], &["trip"], c, Combine::All);
            assert!(only_tags.matches(None, &tags(&["trip"])));
        }
    }

    #[test]
    fn empty_scope_matches_nothing() {
        for c in [Combine::All, Combine::Any] {
            let s = scope(&[], &[], c, Combine::All);
            assert!(s.is_empty());
            assert!(!s.matches(Some("food"), &tags(&["x"])));
            assert!(!s.matches(None, &[]));
            assert!(!s.matches_if_category_resolved(&[]));
        }
    }

    #[test]
    fn category_resolution_hypothetical_respects_tags() {
        let both_all = scope(&["food"], &["trip"], Combine::All, Combine::All);
        assert!(both_all.matches_if_category_resolved(&tags(&["trip"])));
        assert!(!both_all.matches_if_category_resolved(&[]));
        let tags_only = scope(&[], &["trip"], Combine::All, Combine::All);
        assert!(!tags_only.matches_if_category_resolved(&tags(&["trip"])));
    }
}
