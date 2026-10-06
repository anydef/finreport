//! Kafka container for the integration suite.
//!
//! No `redpanda` module exists in `testcontainers-modules` (checked against
//! 0.11 and the current community crate), so this uses its `apache::Kafka`
//! (KRaft, single broker) instead of hand-rolling the dynamic-port /
//! advertised-listener dance a bespoke Redpanda `GenericImage` would need.
//! Both speak the same wire protocol our `rdkafka` client and the projector
//! use, so this is a faithful stand-in for the Redpanda broker
//! `docker-compose.local.yml` runs for interactive dev (§7) — the topics and
//! headers are what's under test, not the broker implementation.

use std::time::Duration;

use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::ClientConfig;
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::kafka::apache::{Kafka, KAFKA_PORT};

use webapp::kafka::envelope::{
    TOPIC_ACCOUNT, TOPIC_ACCOUNT_BALANCE, TOPIC_IMPORT_WATERMARK, TOPIC_TRANSACTION,
};

/// A running Kafka broker with the four finreport topics already created
/// (same names as `terraform/kafka/main.tf` and the local Redpanda
/// topic-init, §7 — partitions/cleanup policy don't matter for a test broker
/// that is thrown away afterwards).
pub struct TestKafka {
    _container: ContainerAsync<Kafka>,
    bootstrap_servers: String,
}

impl TestKafka {
    pub async fn start() -> Self {
        let container = Kafka::default()
            .start()
            .await
            .expect("start Kafka testcontainer");

        let host = container.get_host().await.expect("Kafka testcontainer host");
        let port = container
            .get_host_port_ipv4(KAFKA_PORT)
            .await
            .expect("Kafka testcontainer mapped port");
        let bootstrap_servers = format!("{host}:{port}");

        let this = Self {
            _container: container,
            bootstrap_servers,
        };
        this.create_topics().await;
        this
    }

    /// `host:port` for `rdkafka::ClientConfig::set("bootstrap.servers", …)`
    /// or `APP_kafka_brokers` on a binary under test.
    pub fn bootstrap_servers(&self) -> &str {
        &self.bootstrap_servers
    }

    async fn create_topics(&self) {
        let admin: AdminClient<DefaultClientContext> = ClientConfig::new()
            .set("bootstrap.servers", &self.bootstrap_servers)
            .create()
            .expect("create Kafka admin client");

        let topics = [
            TOPIC_ACCOUNT,
            TOPIC_ACCOUNT_BALANCE,
            TOPIC_TRANSACTION,
            TOPIC_IMPORT_WATERMARK,
        ]
        .map(|name| NewTopic::new(name, 1, TopicReplication::Fixed(1)));

        admin
            .create_topics(&topics, &AdminOptions::new().request_timeout(Some(Duration::from_secs(10))))
            .await
            .expect("create finreport topics on Kafka testcontainer");
    }
}
