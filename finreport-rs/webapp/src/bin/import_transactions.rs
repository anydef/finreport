use comdirect_rs::comdirect::accounts::{get_account_transactions, get_accounts};
use comdirect_rs::comdirect::session::{load_comdirect_session, refresh_comdirect_session};
use comdirect_rs::comdirect::session_client::Session;
use dotenv::dotenv;
use entities::{account, account_balance};
use entity::entities;
use sea_orm::sea_query::OnConflict;
use sea_orm::{DbConn, EntityTrait, Set, Unchanged};
use secrecy::ExposeSecret;
use std::error::Error;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;
use utils::settings::Settings;
use webapp::db::seaql;

// --- Loop tuning -------------------------------------------------------------

const REFRESH_INTERVAL: Duration = Duration::from_secs(8 * 60); // 8 min
const IMPORT_INTERVAL: Duration = Duration::from_secs(4 * 3600); // 4 h
const MAX_BOOTSTRAP_ATTEMPTS: u32 = 6;

/// Exponential-ish backoff between failed bootstrap attempts, capped at 1h.
/// 10m → 20m → 40m → 60m → 60m → 60m  (6 total attempts).
fn bootstrap_backoff(attempt: u32) -> Duration {
    let minutes = match attempt {
        0 => 10,
        1 => 20,
        2 => 40,
        _ => 60,
    };
    Duration::from_secs(minutes * 60)
}

// --- Top-level state machine -------------------------------------------------

enum LoopState {
    /// Acquire a Comdirect session (load existing + refresh, or full OAuth + TAN).
    Bootstrap { attempt: u32 },
    /// Sleep before retrying bootstrap.
    BackoffBeforeBootstrap { delay: Duration, attempt: u32 },
    /// Steady state: a valid session, scheduled refresh and import.
    Run {
        session: Session,
        next_refresh: Instant,
        next_import: Instant,
    },
    /// Permanent failure (e.g. TAN approval repeatedly missed). Exit non-zero;
    /// the container's restart policy will start a fresh run.
    Terminated,
}

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

    info!("[startup] Connecting to database");
    let conn: DbConn =
        seaql::init_db(client_settings.database_url.expose_secret()).await?;
    info!("[startup] Database connected, migrations applied.");

    let mut state = LoopState::Bootstrap { attempt: 0 };
    loop {
        state = match state {
            LoopState::Bootstrap { attempt } => {
                info!(
                    attempt = attempt + 1,
                    max = MAX_BOOTSTRAP_ATTEMPTS,
                    "[bootstrap] starting"
                );
                match load_comdirect_session(client_settings.clone()).await {
                    Ok(session) => {
                        info!("[bootstrap] session acquired");
                        // Import immediately on first successful bootstrap so
                        // the user sees data within seconds of approving TAN.
                        LoopState::Run {
                            session,
                            next_refresh: Instant::now() + REFRESH_INTERVAL,
                            next_import: Instant::now(),
                        }
                    }
                    Err(e) => {
                        let next_attempt = attempt + 1;
                        if next_attempt >= MAX_BOOTSTRAP_ATTEMPTS {
                            error!(
                                ?e,
                                max = MAX_BOOTSTRAP_ATTEMPTS,
                                "[bootstrap] exhausted attempts; exiting"
                            );
                            LoopState::Terminated
                        } else {
                            let delay = bootstrap_backoff(attempt);
                            warn!(
                                ?e,
                                retry_in_min = delay.as_secs() / 60,
                                "[bootstrap] failed; will retry"
                            );
                            LoopState::BackoffBeforeBootstrap {
                                delay,
                                attempt: next_attempt,
                            }
                        }
                    }
                }
            }

            LoopState::BackoffBeforeBootstrap { delay, attempt } => {
                sleep(delay).await;
                LoopState::Bootstrap { attempt }
            }

            LoopState::Run {
                session,
                next_refresh,
                next_import,
            } => {
                let now = Instant::now();
                if next_import <= now {
                    info!("[import] starting");
                    match run_import(&session, &client_settings, &conn).await {
                        Ok(()) => {
                            let next = Instant::now() + IMPORT_INTERVAL;
                            info!(
                                next_run_min = IMPORT_INTERVAL.as_secs() / 60,
                                "[import] done"
                            );
                            LoopState::Run {
                                session,
                                next_refresh,
                                next_import: next,
                            }
                        }
                        Err(e) => {
                            error!(%e, "[import] failed; re-bootstrapping session");
                            LoopState::Bootstrap { attempt: 0 }
                        }
                    }
                } else if next_refresh <= now {
                    info!("[refresh] refreshing session token");
                    match refresh_comdirect_session(client_settings.clone(), &session).await {
                        Ok(new_session) => {
                            info!("[refresh] done");
                            LoopState::Run {
                                session: new_session,
                                next_refresh: Instant::now() + REFRESH_INTERVAL,
                                next_import,
                            }
                        }
                        Err(e) => {
                            error!(?e, "[refresh] failed; re-bootstrapping session");
                            LoopState::Bootstrap { attempt: 0 }
                        }
                    }
                } else {
                    let wait = next_refresh.min(next_import).saturating_duration_since(now);
                    sleep(wait).await;
                    LoopState::Run {
                        session,
                        next_refresh,
                        next_import,
                    }
                }
            }

            LoopState::Terminated => {
                // Non-zero exit so docker's `restart: always` brings us back.
                std::process::exit(1);
            }
        };
    }
}

// --- Import work -------------------------------------------------------------

async fn run_import(
    session: &Session,
    client_settings: &Settings,
    conn: &DbConn,
) -> Result<(), Box<dyn Error>> {
    let accounts = get_accounts(session.clone(), client_settings.clone()).await?;
    info!(
        count = accounts.accounts.len(),
        "[import] loaded accounts from Comdirect"
    );

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

        match account::Entity::insert(account_orm)
            .on_conflict(
                OnConflict::column(account::Column::AccountId)
                    .do_nothing()
                    .to_owned(),
            )
            .exec(conn)
            .await
        {
            Ok(r) => info!(
                display_id = %account.account.display_id,
                last_insert_id = ?r.last_insert_id,
                "inserted account"
            ),
            Err(err) => error!(
                display_id = %account.account.display_id,
                %err,
                "failed to insert account"
            ),
        }

        let balance_orm = account_balance::ActiveModel {
            account_id: Set(account.account.account_id.to_owned()),
            amount: Set(account.balance.value.parse().unwrap_or(0.0)),
            date: Set(chrono::Local::now().date_naive()),
            ..Default::default()
        };

        match account_balance::Entity::insert(balance_orm)
            .on_conflict(
                OnConflict::columns([
                    account_balance::Column::AccountId,
                    account_balance::Column::Date,
                ])
                .do_nothing()
                .to_owned(),
            )
            .exec(conn)
            .await
        {
            Ok(_) => info!(
                display_id = %account.account.display_id,
                balance = %account.balance.value,
                "inserted balance"
            ),
            Err(e) => error!(
                display_id = %account.account.display_id,
                %e,
                "failed to insert balance"
            ),
        }

        debug!(account_id = %account.account_id, "fetching transactions");
        let transactions =
            get_account_transactions(session.clone(), client_settings.clone(), &account.account)
                .await?;

        for transaction in transactions {
            let transaction_orm = entities::account_transactions::ActiveModel {
                reference: Set(transaction.reference.to_owned()),
                account_id: Set(account.account.account_id.to_owned()),
                booking_status: Set(transaction.booking_status),
                booking_date: Set(transaction.booking_date.parse().unwrap()),
                amount: Set(transaction.amount.value.parse().unwrap_or(0.0)),
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

            match entities::account_transactions::Entity::insert(transaction_orm)
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
                .exec(conn)
                .await
            {
                Ok(_) => debug!(reference = %transaction.reference, "inserted transaction"),
                Err(e) => error!(
                    reference = %transaction.reference,
                    %e,
                    "failed to insert transaction"
                ),
            }
        }
    }

    Ok(())
}
