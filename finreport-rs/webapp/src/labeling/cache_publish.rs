//! The one way an `llm_label_cache` row is written: publish the event to the
//! compacted `finreport.llm-cache` topic, then project it. Shared by the
//! labeler and the purchase-order proposal resolver so neither can write the
//! cache without an event behind it (a projection with no event is lost by
//! `just dev-reset` or any replay, taking every paid-for answer with it).
//!
//! A publish failure is logged at `error` and the projection still runs, the
//! same posture as `publish_and_project_label`: the answer is valid and stays
//! usable this run; only a replay would lose it, and the cache refills.

use sea_orm::{ConnectionTrait, DbErr};
use tracing::{error, warn};

use crate::kafka::labeling::{CacheRecord, ORIGIN_LABELER, TOPIC_LLM_CACHE};
use crate::kafka::producer::EventPublisher;
use crate::labeling::processor::labeling_headers;
use crate::projection::labeling as proj;

/// Publishes `record` (or a tombstone when `None`) under `fingerprint`, then
/// projects it. `publisher` is `None` only where no broker is configured, in
/// which case the row is kept in Postgres alone, with a warning.
pub async fn publish_and_project_cache(
    db: &impl ConnectionTrait,
    publisher: Option<&EventPublisher>,
    fingerprint: &str,
    record: Option<CacheRecord>,
) -> Result<(), DbErr> {
    match publisher {
        Some(publisher) => publish(publisher, fingerprint, record.as_ref()).await,
        None => warn!(%fingerprint, "llm-cache: no Kafka broker, change kept in Postgres only"),
    }
    proj::project_llm_cache(db, fingerprint, record).await
}

async fn publish(publisher: &EventPublisher, fingerprint: &str, record: Option<&CacheRecord>) {
    let headers = labeling_headers(ORIGIN_LABELER);
    let result = match record {
        Some(record) => match serde_json::to_vec(record) {
            Ok(value) => publisher
                .publish_with_headers(TOPIC_LLM_CACHE, fingerprint, &value, headers)
                .await,
            Err(e) => {
                error!(error = %e, %fingerprint, "llm-cache: failed to serialize record, not publishing");
                return;
            }
        },
        None => publisher.publish_tombstone_with_headers(TOPIC_LLM_CACHE, fingerprint, headers).await,
    };
    if let Err(e) = result {
        error!(error = %e, %fingerprint, tombstone = record.is_none(), "llm-cache: failed to publish; projecting anyway, a replay would lose it");
    }
}
