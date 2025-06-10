use comdirect_rs::comdirect::accounts::{get_account_transactions, get_accounts};
use comdirect_rs::comdirect::session::load_comdirect_session;
use dotenv::dotenv;
use entities::{account, account_balance};
use entity::entities;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveModelTrait, DbConn, EntityTrait, Set, Unchanged};
use std::env;
use std::error::Error;
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
            account_type: Unchanged(account.account.account_type.text.to_owned()),
            iban: Unchanged(account.account.iban.to_owned()),
            bic: Unchanged(account.account.bic.to_owned()),
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
            account_id: Set(account.account.account_id.to_owned()),
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
        println!("Account: {:?}", account.account_id);
        let transactions =
            get_account_transactions(session.clone(), client_settings.clone(), &account.account)
                .await?;

        for transaction in transactions {
            let transaction_orm = entities::account_transactions::ActiveModel {
                reference: Set(transaction.reference.to_owned()),
                account_id: Set(account.account.account_id.to_owned()),
                booking_status: Set(transaction.booking_status),
                booking_date: Set(transaction.booking_date.parse().unwrap()),
                amount: Set(transaction.amount.value.parse().unwrap_or_else(|_| 0.0)),
                remitter: Set(transaction.remitter.unwrap_or_default().holder_name),
                deptor: Set(transaction.deptor.unwrap_or_default()),
                creditor: Set(transaction
                    .creditor
                    .unwrap_or_default()
                    .holder_name
                    .to_owned()),
                creditor_id: Set(transaction.direct_debit_creditor_id.unwrap_or_default()),
                creditor_mandate_id: Set(transaction.direct_debit_mandate_id.unwrap_or_default()),
                remittance_info: Set(transaction.remittance_info),
                transaction_type: Set(transaction.transaction_type.text),
                ..Default::default()
            };

            let transaction_res =
                entities::account_transactions::Entity::insert(transaction_orm.to_owned())
                    .on_conflict(
                        OnConflict::column(entities::account_transactions::Column::Reference)
                            .update_columns([
                                entities::account_transactions::Column::BookingStatus,
                                entities::account_transactions::Column::BookingDate,
                                entities::account_transactions::Column::Amount,
                                entities::account_transactions::Column::Remitter,
                                entities::account_transactions::Column::Deptor,
                                entities::account_transactions::Column::Creditor,
                                entities::account_transactions::Column::CreditorId,
                                entities::account_transactions::Column::CreditorMandateId,
                                entities::account_transactions::Column::RemittanceInfo,
                                entities::account_transactions::Column::TransactionType,
                            ])
                            .to_owned(),
                    )
                    .exec(&conn)
                    .await;

            match transaction_res {
                Ok(_) => {
                    println!(
                        "Inserted transaction with reference: {}",
                        transaction.reference.to_owned()
                    );
                }
                Err(e) => {
                    println!(
                        "Failed to insert transaction with reference {}: {}",
                        transaction.reference.to_owned(),
                        e
                    );
                }
            }
        }
    }

    Ok(())
}
