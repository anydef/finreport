//! `labeler` (§2.3) — the four-topic (`transaction`/`user-label`/`rule`/
//! `label-request`) consume → normalize → resolve → compare → publish loop.
//!
//! Hard-selects [`categorizer::provider::fake::FakeProvider`] regardless of
//! `APP_llm_provider`: this binary never calls a paid API (per the task
//! brief — a real-provider wiring is left to a later iteration/WP, not WP3).
//! Refuses to start if the projector lags behind by more than
//! `APP_labeler_max_projection_lag` records (§2.3) — the labeler's own reads
//! of `account`/`transaction` rows must see a reasonably fresh read model.

use std::error::Error;
use std::time::Duration;

use dotenv::dotenv;
use secrecy::ExposeSecret;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use utils::settings::Settings;
use webapp::db::seaql;
use webapp::kafka::producer::EventPublisher;
use webapp::labeling::processor::{check_projection_lag, run, LabelerConfig, LabelingOps};

/// Reads `--until-caught-up` from the process arguments, mirroring
/// `projector`'s own arg parsing (§2.3's CI/replay-and-verify use case
/// applies here too).
fn until_caught_up_arg() -> Result<bool, String> {
    let mut until_caught_up = false;
    for arg in std::env::args().skip(1) {
        if arg == "--until-caught-up" {
            until_caught_up = true;
        } else {
            return Err(format!(
                "unexpected argument {arg:?}; usage: [--until-caught-up]"
            ));
        }
    }
    Ok(until_caught_up)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let until_caught_up = until_caught_up_arg().map_err(|e| {
        error!(%e, "[startup] invalid arguments");
        e
    })?;

    let settings = Settings::from_env()?;
    let brokers = settings.require_kafka_brokers()?.to_string();

    info!("[startup] Connecting to database");
    let db = seaql::init_db_with_migrations(
        settings.require_database_url()?.expose_secret(),
        settings.run_migrations(),
    )
    .await?;
    info!(
        run_migrations = settings.run_migrations(),
        "[startup] Database connected."
    );

    info!(
        max_lag = settings.labeler_max_projection_lag,
        "[startup] checking projector is caught up before the labeler starts"
    );
    if let Err(lagging) =
        check_projection_lag(&db, &brokers, settings.labeler_max_projection_lag).await
    {
        for lag in &lagging {
            error!(
                topic = %lag.topic,
                committed = lag.committed,
                high_watermark = lag.high_watermark,
                "[startup] labeler refusing to start: topic not caught up"
            );
        }
        return Err(format!(
            "labeler refusing to start: {} topic(s) exceed the allowed projection lag \
             (APP_labeler_max_projection_lag={}); let the projector catch up first",
            lagging.len(),
            settings.labeler_max_projection_lag
        )
        .into());
    }
    info!("[startup] projection lag check passed");

    let publisher = EventPublisher::connect(&brokers)?;
    // Hard-selected per the task brief: no API keys, deterministic,
    // offline-friendly. A real provider is future work, not WP3's.
    let provider: Box<dyn categorizer::provider::LabelProvider> =
        Box::new(categorizer::provider::fake::FakeProvider::new());
    let ops = LabelingOps::real();

    let config = LabelerConfig {
        brokers,
        batch_max_records: 200,
        batch_max_wait: Duration::from_millis(500),
        llm_max_requests_per_run: settings.llm_max_requests_per_run,
        prompt_version: settings.prompt_version.clone(),
        llm_min_confidence: settings.llm_min_confidence,
        until_caught_up,
    };

    info!(until_caught_up, "[startup] labeler starting");
    run(db, publisher, provider, ops, config).await?;

    Ok(())
}
