//! §2.3 "the labeler": the four-topic consume → normalize → resolve →
//! compare → publish loop, its own offset rows (`@labeler` suffix), the
//! projector-lag startup guard, the unlabelled sweep and the cost guard.
//!
//! WP0 stub — owned by WP3. Depends on WP0 (this crate's Kafka/entity
//! contracts) and WP2 (`normalize`/`fingerprint`/`resolve`/`rules`/`learn`).
//! Modelled on `webapp::projection::run`/`process_batch`, but for
//! `finreport.{transaction,user-label,rule,label-request}` instead of the
//! three ingest topics.

/// Runs the labeler until every ingest topic is caught up, then exits.
/// Mirrors `projection::run`'s `--until-caught-up` behaviour (§2.3), used by
/// `dev-demo` and the integration tests.
///
/// TODO(WP3): implement per §2.3: normalize via the existing
/// `projection::mapper::MapperRegistry`, resolve via `labeling::resolve`,
/// apply the compare-before-publish rule, honour the projector-lag startup
/// guard and `APP_llm_max_requests_per_run`, and run the unlabelled sweep.
pub async fn run_until_caught_up() -> Result<(), String> {
    todo!("WP3: §2.3 labeler processing loop")
}
