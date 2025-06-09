use comdirect_rs::comdirect::accounts::get_accounts;
use comdirect_rs::comdirect::session::load_comdirect_session;
use comdirect_rs::comdirect::transaction::Transaction;
use dotenv::dotenv;
use entities::{account, account_balance};
use entity::entities;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveModelTrait, DbConn, EntityTrait, Set, Unchanged};
use std::env;
use std::error::Error;
use std::path::Path;
use tokio::fs;
use utils::settings::Settings;
use webapp::db::seaql;

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

    let session = load_comdirect_session(client_settings.clone()).await?;

    let accounts = get_accounts(session.clone(), client_settings.clone()).await?;

    let conn: DbConn = seaql::init_db(&client_settings.database_url).await?;

    for account in accounts.accounts {
        let account_orm = account::ActiveModel {
            account_id: Unchanged(account.account.account_id.clone()),
            display_id: Unchanged(account.account.display_id.to_owned()),
            account_type: Unchanged(account.account.account_type.text),
            iban: Unchanged(account.account.iban),
            bic: Unchanged(account.account.bic),
            institute: Unchanged("COMDIRECT".to_string()),
            ..Default::default()
        };

        let account_res = account::Entity::insert(account_orm.to_owned())
            .on_conflict(
                OnConflict::column(account::Column::AccountId)
                    .do_nothing()
                    .to_owned(),
            )
            .exec(&conn)
            .await;

        match account_res {
            Ok(r) => {
                println!(
                    "Inserted account: {} with ID: {}",
                    account.account.display_id, r.last_insert_id
                );
            }
            Err(err) => {
                println!(
                    "Failed to insert account {}: {}",
                    account.account.display_id, err
                );
            }
        }

        let balance_orm = account_balance::ActiveModel {
            account_id: Set(account.account.account_id),
            amount: Set(account.balance.value.parse().unwrap_or_else(|_| 0.0)),
            date: Set(chrono::Local::now().date_naive()),
            ..Default::default()
        };

        let balance_res = account_balance::Entity::insert(balance_orm.to_owned())
            .on_conflict(
                OnConflict::columns([
                    account_balance::Column::AccountId,
                    account_balance::Column::Date,
                ])
                .do_nothing()
                .to_owned(),
            )
            .exec(&conn)
            .await;

        match balance_res {
            Ok(_) => {
                println!(
                    "Inserted balance for account {}: {}",
                    account.account.display_id, account.balance.value
                );
            }
            Err(e) => {
                println!(
                    "Failed to insert balance for account {}: {}",
                    account.account.display_id, e
                );
            }
        }

        // println!("Account: {:?}", account.account_id);
        // let transactions =
        //     get_account_transactions(session.clone(), client_settings.clone(), &account.account)
        //         .await?;
        //
        // save_transactions_to_file(
        //     &*transactions,
        //     format!("transactions-{}.json", &account.account.display_id),
        // )
        // .await?;
        // save_transactions_to_ndjson(
        //     &*transactions,
        //     format!("transactions-{}.ndjson", &account.account.display_id),
        // )
        // .await?;

        // println!("Transactions: {:?}", transactions.len());
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
