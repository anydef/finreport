//! Keeps a single Comdirect login's session alive: bootstraps it (including
//! the push-TAN approval) and re-loads it periodically.
//!
//! This one drives a single login interactively — pick it with
//! `--account <key>`; the flag may be omitted when only one is configured.
//! (The importer, by contrast, runs every configured account at once.)

use comdirect_rs::comdirect::session::load_comdirect_session;
use dotenv::dotenv;
use std::error::Error;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use utils::settings::Settings;
use webapp::cli::account_arg;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let requested_account = account_arg().map_err(|e| {
        error!(%e, "invalid arguments");
        e
    })?;

    let client_settings = Settings::from_env()?;
    let profile = client_settings
        .select_profile(requested_account.as_deref())
        .map_err(|e| {
            error!(%e, "could not select Comdirect account");
            e
        })?;
    info!(
        account = %profile.key,
        session_file = %profile.save_file_path,
        "initialising session for Comdirect account"
    );

    loop {
        let session_result = load_comdirect_session(&profile).await;
        info!(account = %profile.key, ?session_result, "session result");
        sleep(Duration::from_secs(300)).await;
    }
}
