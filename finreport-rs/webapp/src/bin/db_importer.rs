use std::error::Error;
use comdirect_rs::comdirect::transaction::Transaction;
use webapp::db::{Persistence, DB_URL};
use sqlx::{migrate::MigrateDatabase, Sqlite};
use tokio::fs;


#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let json_content = fs::read_to_string("transactions-102455031500.json").await?;
    let transactions: Vec<Transaction> = serde_json::from_str(&json_content)?;

    if !Sqlite::database_exists(DB_URL).await.unwrap_or(false) {
        println!("Creating database {}", DB_URL);
        match Sqlite::create_database(DB_URL).await {
            Ok(_) => println!("Create db success"),
            Err(error) => panic!("error: {}", error),
        }
    } else {
        println!("Database already exists.");
    }
    save_transactions_to_db(
        &*transactions, DB_URL).await.expect("Failed to save transactions");

    Ok(())
}


async fn save_transactions_to_db(
    transactions: &[Transaction],
    file_path: &str,
) -> Result<(), Box<dyn Error>> {
    let mut persistence = Persistence::new(file_path).await?;
    persistence.insert_transactions(transactions).await?;

    Ok(())
}
