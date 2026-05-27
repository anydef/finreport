use comdirect_rs::comdirect::session::load_comdirect_session;
use dotenv::dotenv;
use std::error::Error;
use std::time::Duration;
use tokio::time::sleep;
use tracing::info;
use tracing_subscriber::EnvFilter;
use utils::settings::Settings;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let settings = config::Config::builder()
        .add_source(
            config::Environment::with_prefix("APP")
                .prefix_separator("_")
                .separator("__"),
        )
        .build()?;
    let client_settings = settings
        .try_deserialize::<Settings>()
        .expect("Could not load application settings");
    loop {
        let session_result = load_comdirect_session(client_settings.clone()).await;
        info!(?session_result, "session result");
        sleep(Duration::from_secs(300)).await;
    }
}
