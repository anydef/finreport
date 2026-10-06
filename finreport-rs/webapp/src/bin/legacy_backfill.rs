//! One-off migration of the renamed `legacy_*` tables onto the regular
//! ingest topics, tagged `origin=legacy-backfill` (§2.8).
//!
//! Stub for WP0 (contracts/schema only): this binary exists so the `[[bin]]`
//! entry in `Cargo.toml` builds. WP1 owns its implementation.

fn main() {
    unimplemented!("legacy-backfill: implemented by WP1 (§2.8)");
}
