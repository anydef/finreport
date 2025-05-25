use dotenv::dotenv;
use finreport::comdirect::accounts::{get_account_transactions, get_accounts};
use finreport::comdirect::session::get_comdirect_session;
use finreport::comdirect::transaction::Transaction;
use finreport::settings::Settings;
use std::env;
use std::error::Error;
use std::path::Path;
use tokio::fs;

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
            get_account_transactions(session.clone(), client_settings.clone(), &account.account)
                .await?;

        save_transactions_to_file(
            &*transactions,
            format!("transactions-{}.json", &account.account.display_id),
        )
        .await?;
        save_transactions_to_ndjson(
            &*transactions,
            format!("transactions-{}.ndjson", &account.account.display_id),
        )
        .await?;

        println!("Transactions: {:?}", transactions.len());
    }

    Ok(())
}

async fn save_transactions_to_file(
    transactions: &[Transaction],
    file_path: impl AsRef<Path>,
) -> Result<(), Box<dyn Error>> {
    // Serialize transactions to JSON
    let json = serde_json::to_string(&transactions)?;

    // Write to file asynchronously
    fs::write(file_path, json).await?;

    Ok(())
}

async fn save_transactions_to_ndjson(
    transactions: &[Transaction],
    file_path: impl AsRef<Path>,
) -> Result<(), Box<dyn Error>> {
    let mut ndjson = String::new();

    // Process each transaction as a separate JSON line
    for transaction in transactions {
        let json_line = serde_json::to_string(transaction)?;
        ndjson.push_str(&json_line);
        ndjson.push('\n');
    }

    // Write to file asynchronously
    fs::write(file_path, ndjson).await?;

    Ok(())
}
