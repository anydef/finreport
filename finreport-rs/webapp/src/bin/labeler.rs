//! `labeler` (§2.3) — WP0 stub. WP3 owns the real implementation: the
//! four-topic consume → normalize → resolve → compare → publish loop, its
//! own `@labeler`-suffixed offset rows, the projector-lag startup guard, the
//! unlabelled sweep and the cost guard. Frozen here only so the `[[bin]]`
//! entry and crate wiring exist for WP3 to build against, per the WP0
//! shared-file protocol (`docs/specs/iteration-2.md` §10).

fn main() {
    eprintln!("labeler: not yet implemented (WP3, see docs/specs/iteration-2.md §2.3)");
    std::process::exit(1);
}
