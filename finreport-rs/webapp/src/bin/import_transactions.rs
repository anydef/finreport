//! Imports balances and transactions for every configured Comdirect login and
//! publishes them to the event log (§2.6). Kafka is the importer's only
//! output now — there is no Postgres write to fall back on, so
//! `APP_kafka_brokers` is required at startup and a publish failure is
//! logged as data loss rather than swallowed (§2.6/§2.7).
//!
//! One process, one task per login: each login runs its own state machine, so
//! it approves its own push-TAN and re-bootstraps its own stale session without
//! holding up the others. `--account <key>` narrows the run to a single login
//! (handy locally); without it every configured account is imported.

use comdirect_rs::comdirect::accounts::{get_account_transactions_since_raw, get_accounts_raw};
use comdirect_rs::comdirect::session::{load_comdirect_session, refresh_comdirect_session};
use comdirect_rs::comdirect::session_client::Session;
use comdirect_rs::comdirect::transaction::ImportStop;
use dotenv::dotenv;
use std::collections::HashMap;
use std::error::Error;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tokio::time::sleep;
use tracing::{debug, error, info, info_span, warn, Instrument};
use tracing_subscriber::EnvFilter;
use utils::settings::{ComdirectProfile, Settings};
use webapp::cli::account_arg;
use webapp::kafka::envelope::{RecordMeta, CURRENT_SCHEMA_VERSION, ORIGIN_SOURCE, SOURCE_COMDIRECT};
use webapp::kafka::events::split_account_element;
use webapp::kafka::producer::EventPublisher;
use webapp::kafka::watermark::{load_watermarks, publish_watermark, Watermark};
use webapp::kafka::{TOPIC_ACCOUNT, TOPIC_ACCOUNT_BALANCE, TOPIC_TRANSACTION};

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
    // The event log is the only output this binary has; an importer that
    // cannot publish has nowhere to put what it fetches (§2.6).
    let brokers = client_settings.require_kafka_brokers()?.to_string();

    let profiles = match requested_account.as_deref() {
        Some(key) => vec![client_settings.select_profile(Some(key))?],
        None => client_settings.profiles()?,
    };

    // One task per login. They run concurrently — each approves its own TAN and
    // keeps its own session file.
    let mut accounts = JoinSet::new();
    for profile in profiles {
        info!(
            account = %profile.key,
            account_name = profile.name.as_deref().unwrap_or("<unnamed>"),
            session_file = %profile.save_file_path,
            "[startup] starting importer for Comdirect account"
        );
        let span = info_span!("account", key = %profile.key);
        let brokers = brokers.clone();
        accounts.spawn(async move { run_account(profile, brokers).instrument(span).await });
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
async fn run_account(profile: ComdirectProfile, brokers: String) -> String {
    let publisher = match EventPublisher::connect(&brokers) {
        Ok(publisher) => publisher,
        Err(e) => {
            error!(%e, "[startup] could not construct the Kafka producer; giving up");
            return profile.key;
        }
    };

    // Resume points, so an import only publishes what the log lacks. An empty
    // map means "import everything", which is the first-run path.
    let mut watermarks = {
        let brokers = brokers.clone();
        match tokio::task::spawn_blocking(move || load_watermarks(&brokers)).await {
            Ok(Ok(watermarks)) => {
                info!(accounts = watermarks.len(), "[startup] loaded resume points");
                watermarks
            }
            Ok(Err(e)) => {
                warn!(%e, "[startup] could not read resume points; importing full history");
                HashMap::new()
            }
            Err(e) => {
                warn!(%e, "[startup] resume-point read panicked; importing full history");
                HashMap::new()
            }
        }
    };

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
                    match run_import(&session, &profile, &publisher, &mut watermarks).await {
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
    publisher: &EventPublisher,
    watermarks: &mut HashMap<String, Watermark>,
) -> Result<(), Box<dyn Error>> {
    let accounts = get_accounts_raw(session.clone(), profile).await?;
    let imported_at = chrono::Utc::now().to_rfc3339();
    info!(
        account = %profile.key,
        count = accounts.accounts.len(),
        "[import] loaded accounts from Comdirect"
    );

    for element in accounts.accounts {
        let account = &element.parsed;
        let account_id = account.account.account_id.clone();
        // `source_account_id` is sent on every topic, including `account`
        // (redundant there — the payload already names it — but uniform, §2.2).
        let meta = RecordMeta {
            source: SOURCE_COMDIRECT,
            source_account_id: Some(account_id.as_str()),
            origin: ORIGIN_SOURCE,
            schema_version: CURRENT_SCHEMA_VERSION,
            imported_at: &imported_at,
            comdirect_account_key: &profile.key,
            comdirect_account_name: profile.name.as_deref(),
        };

        // Publish the bank's own bytes for this account and its balance. The
        // response bundles both, but they belong on different topics, so they
        // are sliced out of the original payload rather than re-encoded.
        // Best-effort: a lost account/balance snapshot is replaced by the
        // next import cycle, unlike a transaction, which exists exactly once.
        match split_account_element(&element.raw) {
            Ok((account_json, balance_json)) => {
                publisher
                    .publish_best_effort(
                        TOPIC_ACCOUNT,
                        &account_id,
                        account_json.get().as_bytes(),
                        &meta,
                    )
                    .await;
                publisher
                    .publish_best_effort(
                        TOPIC_ACCOUNT_BALANCE,
                        &account_id,
                        balance_json.get().as_bytes(),
                        &meta,
                    )
                    .await;
            }
            Err(e) => warn!(
                display_id = %account.account.display_id,
                %e, "could not split account payload; not published"
            ),
        }

        // Resume where the log left off. With no watermark (first run) this
        // is an unrestricted fetch.
        let stop = watermarks
            .get(&account_id)
            .map(|w| ImportStop {
                last_reference: w.last_reference.clone(),
                last_booking_date: w.last_booking_date,
            })
            .unwrap_or_default();
        debug!(
            account_id = %account_id,
            resume_from = ?stop.last_booking_date,
            "fetching transactions"
        );
        let transactions =
            get_account_transactions_since_raw(session.clone(), profile, &account.account, &stop)
                .await?;

        // Newest by booking date, not by position: the API's ordering is an
        // assumption the client guards but does not rely on.
        let newest = transactions
            .iter()
            .filter_map(|t| {
                t.parsed
                    .booking_date
                    .parse::<chrono::NaiveDate>()
                    .ok()
                    .map(|date| (date, t.parsed.reference.clone()))
            })
            .max_by(|a, b| a.0.cmp(&b.0));

        // A publish failure here is data loss (there is no DB copy to fall
        // back on), so every transaction's outcome is tracked: if any fails,
        // the watermark for this cycle is not advanced — advancing it past an
        // unpublished transaction would lose it permanently (§2.6/§2.7). The
        // loop still attempts every remaining transaction rather than
        // aborting the batch, so a single blip costs one retry next cycle,
        // not the whole account's progress.
        let mut all_published = true;
        for raw_transaction in &transactions {
            let transaction = &raw_transaction.parsed;
            match publisher
                .publish(
                    TOPIC_TRANSACTION,
                    &transaction.reference,
                    raw_transaction.raw.get().as_bytes(),
                    &meta,
                )
                .await
            {
                Ok(()) => debug!(reference = %transaction.reference, "published transaction"),
                Err(e) => {
                    error!(
                        reference = %transaction.reference,
                        %e,
                        "failed to publish transaction; watermark will not advance this cycle"
                    );
                    all_published = false;
                }
            }
        }

        // Advance the resume point only when every transaction in this batch
        // published successfully and this pass actually saw something newer.
        // An empty fetch means the log is already current, and moving the
        // watermark backwards would re-publish history next run.
        if let (true, Some((booking_date, reference))) = (all_published, newest) {
            let already_current = watermarks
                .get(&account_id)
                .and_then(|w| w.last_booking_date)
                .is_some_and(|previous| previous > booking_date);

            if !already_current {
                let watermark = Watermark {
                    account_id: account_id.clone(),
                    last_booking_date: Some(booking_date),
                    last_reference: Some(reference),
                    updated_at: chrono::Utc::now(),
                };
                publish_watermark(publisher, &watermark, &meta).await;
                debug!(
                    account_id = %account_id,
                    %booking_date,
                    "advanced resume point"
                );
                watermarks.insert(account_id.clone(), watermark);
            }
        }
    }

    Ok(())
}
