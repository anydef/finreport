//! Shared testcontainers harness for the integration suite (§8, §9 WP6).
//!
//! Every file under `tests/support/` is a plain module, never a standalone
//! test binary — Cargo's default test discovery only picks up `tests/*.rs`,
//! so a test file anywhere else (WP3's `tests/projector_postgres.rs` /
//! `tests/projector_kafka.rs`, `tests/harness_smoke.rs`, …) pulls this in
//! with `mod support;` and gets Postgres + Kafka helpers without
//! re-implementing container plumbing. WP3's projector suite originally
//! carried its own, near-identical Postgres/Redpanda/fixture-loading harness
//! (written before this one existed); merging the two branches folded it
//! into this shared one rather than keeping two copies — WP3's test files
//! were adjusted to call it instead (see their module docs).
//!
//! Entirely behind the `integration` cargo feature (§9 shared-file
//! protocol): `just test` never compiles this, `just test-integration` does.
//! That feature gate belongs on every *caller*, not here — this module has
//! no side effects of its own until one of its functions runs.
//!
//! Every test binary that pulls this file in via `#[path = "support/mod.rs"]`
//! gets its own separate copy compiled, and no single binary uses every
//! helper here (e.g. `harness_smoke.rs` never touches `load_fixture_records`,
//! `projector_postgres.rs` never touches `TestKafka`) — `dead_code`/
//! `unused_imports` are allowed crate-wide for that reason, not to hide a
//! real bug.
#![allow(dead_code, unused_imports)]

pub mod fixtures;
pub mod kafka;
pub mod migrate;
pub mod postgres;

pub use fixtures::{
    load_fixture_publish_entries, load_fixture_records, publish_fixture_corpus, FixtureRecord,
};
pub use kafka::TestKafka;
pub use migrate::run_migrations;
pub use postgres::TestPostgres;
