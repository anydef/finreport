//! `category-seed` (§3, §7) — WP0 stub. WP3 owns the real implementation:
//! reading `prompts/taxonomy.json` and idempotently publishing one
//! `finreport.category` record per node (same slugs ⇒ same UUIDv5 ids),
//! never deleting a user-created category. Frozen here only so the
//! `[[bin]]` entry and crate wiring exist for WP3 to build against, per the
//! WP0 shared-file protocol (`docs/specs/iteration-2.md` §10).

fn main() {
    eprintln!(
        "category-seed: not yet implemented (WP3, see docs/specs/iteration-2.md §3 \"Seed taxonomy\")"
    );
    std::process::exit(1);
}
