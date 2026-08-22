//! Imports balances and transactions for every configured Comdirect login.
//!
//! One process, one task per login: each login runs its own state machine, so
//! it approves its own push-TAN and re-bootstraps its own stale session without
//! holding up the others. `--account <key>` narrows the run to a single login
//! (handy locally); without it every configured account is imported.

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
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tokio::time::sleep;
use tracing::{debug, error, info, info_span, warn, Instrument};
use tracing_subscriber::EnvFilter;
use utils::settings::{ComdirectProfile, Settings};
use webapp::cli::account_arg;
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

    let requested_account = account_arg().map_err(|e| {
        error!(%e, "[startup] invalid arguments");
        e
    })?;

    let client_settings = Settings::from_env()?;
    let profiles = match requested_account.as_deref() {
        Some(key) => vec![client_settings.select_profile(Some(key))?],
        None => client_settings.profiles()?,
    };

    info!("[startup] Connecting to database");
    let conn: Arc<DbConn> =
        Arc::new(seaql::init_db(client_settings.database_url.expose_secret()).await?);
    info!("[startup] Database connected, migrations applied.");

    // One task per login. They run concurrently — each approves its own TAN and
    // keeps its own session file — and the connection pool is shared.
    let mut accounts = JoinSet::new();
    for profile in profiles {
        info!(
            account = %profile.key,
            account_name = profile.name.as_deref().unwrap_or("<unnamed>"),
            session_file = %profile.save_file_path,
            "[startup] starting importer for Comdirect account"
        );
        let span = info_span!("account", key = %profile.key);
        let conn = Arc::clone(&conn);
        accounts.spawn(async move { run_account(profile, conn).instrument(span).await });
    }

    // Each task only returns once that login has failed for good; the others
    // keep going. Exit non-zero once they have all given up so the container's
    // restart policy takes over.
    let mut terminated = Vec::new();
    while let Some(joined) = accounts.join_next().await {
        match joined {
            Ok(key) => {
                error!(account = %key, "[shutdown] account gave up permanently");
                terminated.push(key);
            }
            Err(e) => error!(?e, "[shutdown] account task panicked"),
        }
    }

    error!(
        accounts = ?terminated,
        "[shutdown] every configured account has stopped importing; exiting"
    );
    std::process::exit(1);
}

/// Drives one Comdirect login forever: session bootstrap (including the TAN
/// approval), periodic token refresh and periodic import. Returns the account
/// key only when that login has failed for good.
async fn run_account(profile: ComdirectProfile, conn: Arc<DbConn>) -> String {
    let mut state = LoopState::Bootstrap { attempt: 0 };
    loop {
        state = match state {
            LoopState::Bootstrap { attempt } => {
                info!(
                    attempt = attempt + 1,
                    max = MAX_BOOTSTRAP_ATTEMPTS,
                    "[bootstrap] starting"
                );
                match load_comdirect_session(&profile).await {
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
                    match run_import(&session, &profile, &conn).await {
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
                    match refresh_comdirect_session(&profile, &session).await {
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

            // Only this login stops; the other accounts carry on importing.
            LoopState::Terminated => return profile.key,
        };
    }
}

// --- Import work -------------------------------------------------------------

async fn run_import(
    session: &Session,
    profile: &ComdirectProfile,
    conn: &DbConn,
) -> Result<(), Box<dyn Error>> {
    let accounts = get_accounts(session.clone(), profile).await?;
    info!(
        account = %profile.key,
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
            account_name: Set(profile.name.clone()),
            ..Default::default()
        };

        match account::Entity::insert(account_orm)
            .on_conflict(
                // Only the name is refreshed on re-import, so renaming a login
                // in the config lands on its accounts the next time it runs.
                OnConflict::column(account::Column::AccountId)
                    .update_column(account::Column::AccountName)
                    .to_owned(),
            )
            .exec(conn)
            .await
        {
            Ok(r) => info!(
                display_id = %account.account.display_id,
                account_name = profile.name.as_deref().unwrap_or("<unnamed>"),
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

        // One balance per account per day. Re-running the import within the
        // same day hits DO NOTHING, which returns zero rows — expected, not an
        // error, so this uses exec_without_returning rather than exec(), whose
        // RETURNING clause turns a skipped row into `RecordNotInserted`.
        match account_balance::Entity::insert(balance_orm)
            .on_conflict(
                OnConflict::columns([
                    account_balance::Column::AccountId,
                    account_balance::Column::Date,
                ])
                .do_nothing()
                .to_owned(),
            )
            .exec_without_returning(conn)
            .await
        {
            Ok(0) => debug!(
                display_id = %account.account.display_id,
                "balance for today already recorded"
            ),
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
            get_account_transactions(session.clone(), profile, &account.account).await?;

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
