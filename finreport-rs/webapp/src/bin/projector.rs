//! Consumes the ingest topics and projects them into the read model (§2.3).
//!
//! No consumer group: `assign()` resumes from the `next_offset`s stored in
//! `projection_offset` (scoped by `APP_projection_group`), which `process_batch` commits alongside each batch's
//! rows in the same transaction. `--until-caught-up` exits once every ingest
//! topic's high watermark has been reached, instead of polling forever —
//! mainly useful for CI/replay-and-verify runs.

use std::error::Error;

use dotenv::dotenv;
use entity::entities::app_user;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use secrecy::ExposeSecret;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use utils::settings::Settings;
use webapp::db::seaql;
use webapp::projection::{run, ProjectorConfig};

/// Reads `--until-caught-up` from the process arguments. No other flag is
/// accepted; anything else is a startup error rather than a silent no-op.
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

    let default_owner = match &settings.projector_default_owner {
        Some(username) => {
            let user = app_user::Entity::find()
                .filter(app_user::Column::Username.eq(username.as_str()))
                .one(&db)
                .await?
                .ok_or_else(|| {
                    format!(
                        "APP_projector_default_owner={username:?} does not match any app_user; \
                         create it first with `user-admin`"
                    )
                })?;
            info!(%username, "[startup] newly projected accounts will be linked to this user");
            Some(user.id)
        }
        None => {
            info!("[startup] APP_projector_default_owner unset; accounts stay unlinked until an explicit `user-admin link`");
            None
        }
    };

    let mut config = ProjectorConfig::new(brokers, settings.projection_group.clone());
    config.default_owner = default_owner;
    config.until_caught_up = until_caught_up;

    info!(until_caught_up, "[startup] projector starting");
    run(db, config).await?;

    Ok(())
}
