//! `labeler` (§2.3) — the four-topic (`transaction`/`user-label`/`rule`/
//! `label-request`) consume → normalize → resolve → compare → publish loop.
//!
//! Builds its [`categorizer::provider::LabelProvider`] through WP1's
//! `categorizer::factory::build_provider` (`APP_llm_provider`, defaulting to
//! `fake` so local dev needs no API key).
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
use webapp::labeling::processor::{check_projection_lag, effective_sweep_interval, run, LabelerConfig, LabelingOps};

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
    info!(group_id = %settings.projection_group, "[startup] projection group (offsets are scoped to it; change APP_projection_group to replay from the beginning)");
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
        check_projection_lag(&db, &settings.projection_group, &brokers, settings.labeler_max_projection_lag).await
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
    let provider: Box<dyn categorizer::provider::LabelProvider> =
        categorizer::factory::build_provider(&settings)?;
    let ops = LabelingOps::real(
        settings.rule_learn_min_observations,
        settings.rule_learn_min_user_observations,
        settings.rule_auto_approve_threshold,
    );

    let config = LabelerConfig {
        brokers,
        group_id: settings.projection_group.clone(),
        batch_max_records: 200,
        batch_max_wait: Duration::from_millis(500),
        llm_max_requests_per_run: settings.llm_max_requests_per_run,
        prompt_version: settings.prompt_version.clone(),
        llm_min_confidence: settings.llm_min_confidence,
        until_caught_up,
        sweep_interval: effective_sweep_interval(settings.labeler_sweep_interval_secs, until_caught_up),
    };

    info!(until_caught_up, "[startup] labeler starting");
    run(db, publisher, provider, ops, config).await?;

    Ok(())
}
