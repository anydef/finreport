//! Publish-then-upsert plumbing (§2.1) shared by every WP4 mutation that
//! writes a `user-label`, `rule`, `category` or `label-request` record.
//!
//! Every mutation publishes to Kafka (awaiting the broker ack — a publish
//! failure fails the mutation) and applies the identical upsert to its own
//! projection table in the same request, both carrying the same `revision`.
//! This builds the §2.2 header set directly rather than going through
//! `envelope::RecordMeta`, which used to be comdirect-shaped (it *required* a
//! `comdirect_account_key`). That is no longer true — `RecordMeta` now takes a
//! `SourceContext`, and `Unattributed` is exactly this case — so these topics
//! could go through it and would then also carry the `source` and
//! `imported_at` headers they currently lack. Deliberately not done here: it
//! changes what is written to five live compacted topics, so it is its own
//! slice with its own replay check (tracked in `docs/TODO.md`).
//! `origin` is always [`ORIGIN_USER`] here, since every WP4-published record
//! is human-edited, never labeler output.

use async_graphql::ErrorExtensions;
use rdkafka::message::{Header, OwnedHeaders};
use std::sync::Arc;

use crate::kafka::envelope::{HEADER_ORIGIN, HEADER_SCHEMA_VERSION};
use crate::kafka::labeling::{CURRENT_SCHEMA_VERSION, ORIGIN_USER};
use crate::kafka::producer::EventPublisher;

/// `extensions.code` for a mutation that needs to publish but the server has
/// no `APP_kafka_brokers` configured (e.g. local `dev-be`).
pub fn kafka_unavailable_error() -> async_graphql::Error {
    async_graphql::Error::new(
        "this mutation publishes to Kafka, but no broker is configured (APP_kafka_brokers)",
    )
    .extend_with(|_, e| e.set("code", "KAFKA_UNAVAILABLE"))
}

fn headers() -> OwnedHeaders {
    OwnedHeaders::new()
        .insert(Header {
            key: HEADER_ORIGIN,
            value: Some(ORIGIN_USER),
        })
        .insert(Header {
            key: HEADER_SCHEMA_VERSION,
            value: Some(CURRENT_SCHEMA_VERSION.to_string().as_str()),
        })
}

/// Publishes `value` (already serialized JSON) on `topic` under `key`,
/// awaiting the broker ack. Surfaces a publish failure as a GraphQL error —
/// per §2.1, a mutation must not apply its projection upsert when the
/// publish itself failed.
pub async fn publish_event(
    publisher: &Arc<EventPublisher>,
    topic: &str,
    key: &str,
    value: &[u8],
) -> async_graphql::Result<()> {
    publisher
        .publish_with_headers(topic, key, value, headers())
        .await
        .map_err(|e| {
            async_graphql::Error::new(format!("failed to publish to {topic}: {e}"))
                .extend_with(|_, ext| ext.set("code", "KAFKA_PUBLISH_FAILED"))
        })
}

/// Publishes a tombstone (null value) on `topic` under `key`, awaiting the
/// broker ack: compaction's "this entity no longer exists", which the
/// projections turn into a delete.
pub async fn publish_tombstone(
    publisher: &Arc<EventPublisher>,
    topic: &str,
    key: &str,
) -> async_graphql::Result<()> {
    publisher
        .publish_tombstone_with_headers(topic, key, headers())
        .await
        .map_err(|e| {
            async_graphql::Error::new(format!("failed to publish tombstone to {topic}: {e}"))
                .extend_with(|_, ext| ext.set("code", "KAFKA_PUBLISH_FAILED"))
        })
}
