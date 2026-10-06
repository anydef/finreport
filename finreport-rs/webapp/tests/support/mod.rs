//! Shared testcontainers harness for the integration suite (§8, §9 WP6).
//!
//! Every file under `tests/support/` is a plain module, never a standalone
//! test binary — Cargo's default test discovery only picks up `tests/*.rs`,
//! so a test file anywhere else (WP3's `tests/projector.rs`, WP4's
//! `tests/graphql.rs`, …) pulls this in with `mod support;` and gets Postgres
//! + Kafka helpers without re-implementing container plumbing.
//!
//! Entirely behind the `integration` cargo feature (§9 shared-file
//! protocol): `just test` never compiles this, `just test-integration` does.
//! That feature gate belongs on every *caller*, not here — this module has
//! no side effects of its own until one of its functions runs.

pub mod fixtures;
pub mod kafka;
pub mod migrate;
pub mod postgres;

// Re-exported for other WPs' test files (`tests/projector.rs`,
// `tests/graphql.rs`, …) to `use support::{...}` — `harness_smoke.rs` itself
// only exercises a subset, so `FixtureRecord`/`run_migrations` look unused
// from this crate's own test binary alone.
#[allow(unused_imports)]
pub use fixtures::{publish_fixture_corpus, FixtureRecord};
pub use kafka::TestKafka;
#[allow(unused_imports)]
pub use migrate::run_migrations;
pub use postgres::TestPostgres;
