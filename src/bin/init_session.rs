use dotenv::dotenv;
use finreport::comdirect::session::Session;
use finreport::settings::Settings;
use reqwest::header::HeaderMap;
use std::env;
use std::error::Error;
use finreport::comdirect::balance::accounts_balances;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenv().ok();
    unsafe {
        env::set_var("RUST_LOG", "reqwest=trace");
    }
    env_logger::init();

    let settings = config::Config::builder()
        .add_source(
            config::Environment::with_prefix("APP")
                .prefix_separator("_")
                .separator("__"),
        )
        .build()?;

    let client = reqwest::Client::builder()
        .connection_verbose(false)
        .build()?;

    let oauth_settings = settings
        .try_deserialize::<Settings>()
        .expect("Could not load application settings");

    let mut oauth_session = Session::new(oauth_settings, client);
    oauth_session.login().await;

    let balances = accounts_balances(oauth_session).await;
    match balances {
        Ok(balances) => {
            println!("Balances: {:?}", balances);
        }
        Err(e) => {
            println!("Error fetching balances: {:?}", e);
        }
    }

    Ok(())
}


