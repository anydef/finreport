use dotenv::dotenv;
use finreport::comdirect::accounts::{get_account_transactions, get_accounts};
use finreport::comdirect::session::get_comdirect_session;
use finreport::settings::Settings;
use std::env;
use std::error::Error;

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
    let client_settings = settings
        .try_deserialize::<Settings>()
        .expect("Could not load application settings");

    let session = get_comdirect_session(client_settings.clone()).await?;

    let accounts = get_accounts(session.clone(), client_settings.clone()).await?;

    for account in accounts.accounts {
        println!("Account: {:?}", account.account_id);
        let transactions =
            get_account_transactions(session.clone(), client_settings.clone(), account.account).await?;
        println!("Transactions: {:?}", transactions.len());
    }

    Ok(())
}