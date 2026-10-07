//! Shared seeding/connection helpers for the `integration`-gated tests
//! (§8/§9). Connects to the throwaway `finreport-wp4-pg` container
//! (`docker run --name finreport-wp4-pg -p 55435:5432 ...` — see the WP4
//! final report for the exact invocation) rather than spinning up
//! `testcontainers` per test, per the task's explicit instruction. Every
//! seed id is a fresh `Uuid::new_v4()`, so tests sharing one database stay
//! independent without needing per-test truncation.

#![allow(dead_code)]

use chrono::Utc;
use entity::entities::{account, app_user, transaction, user_account};
use rust_decimal::Decimal;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use secrecy::SecretString;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;
use webapp::auth;
use webapp::db::seaql;

const TEST_DATABASE_URL: &str = "postgres://postgres:postgres@127.0.0.1:55435/finreport_wp4";

/// Connects fresh per test (rather than sharing one pool via a `static`):
/// `sqlx`'s pool ties its background tasks to the Tokio runtime it was
/// created on, and each `#[tokio::test]` spins up its own runtime, so a
/// shared pool intermittently hits `ConnectionAcquire(Timeout)` once an
/// earlier test's runtime has shut down. `Migrator::up` is a fast no-op
/// once the migrations table is current, so paying it per test is cheap.
pub async fn db() -> Arc<DatabaseConnection> {
    Arc::new(
        seaql::init_db(TEST_DATABASE_URL)
            .await
            .expect("connect to finreport-wp4-pg on 127.0.0.1:55435 — is the container up?"),
    )
}

pub fn dummy_settings() -> Arc<utils::settings::Settings> {
    Arc::new(utils::settings::Settings {
        oauth_url: None,
        url: None,
        save_file_path: None,
        database_url: Some(SecretString::from(TEST_DATABASE_URL.to_string())),
        kafka_brokers: None,
        cookie_secure: false,
        allowed_origins: "http://localhost:5173".to_string(),
        session_ttl_days: 30,
        projector_default_owner: None,
        llm_provider: "fake".to_string(),
        anthropic_api_key: None,
        llm_api_key: None,
        llm_base_url: None,
        llm_model: None,
        llm_timeout_ms: 20_000,
        llm_min_confidence: 0.5,
        llm_max_requests_per_run: 200,
        prompt_version: "2".to_string(),
        rule_learn_min_observations: 3,
        rule_auto_approve_threshold: 0.9,
        labeler_max_projection_lag: 0,
        accounts: BTreeMap::new(),
        account_name: None,
        client_id: None,
        client_secret: None,
        zugangsnummer: None,
        pin: None,
    })
}

/// Inserts a fresh `app_user` row with a real Argon2id hash (so login tests
/// exercise the real verify path too), lowercased like `authenticate` does.
/// Returns the generated id and the actual (uniqueified) username stored,
/// since that's what a caller needs to log in as.
pub async fn seed_user(db: &DatabaseConnection, username: &str, password: &str) -> (Uuid, String) {
    let user_id = Uuid::new_v4();
    // `username` is unique; suffix with the row's own id so reruns against
    // a not-yet-cleaned-up container (or a shared CI database) don't
    // collide on a human-readable prefix like "alice-scope-test".
    let unique_username = format!("{}-{}", username.to_lowercase(), user_id);
    app_user::ActiveModel {
        id: Set(user_id),
        username: Set(unique_username.clone()),
        password_hash: Set(
            auth::hash_password(&SecretString::from(password.to_string()))
                .expect("hash password"),
        ),
        display_name: Set(Some(username.to_string())),
        disabled: Set(false),
        created_at: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert app_user");
    (user_id, unique_username)
}

/// Inserts a fresh `account` row with no Comdirect-specific fields set.
pub async fn seed_account(db: &DatabaseConnection, currency: &str, label: &str) -> Uuid {
    let account_id = Uuid::new_v4();
    account::ActiveModel {
        id: Set(account_id),
        source: Set("test".to_string()),
        external_id: Set(Uuid::new_v4().to_string()),
        display_id: Set(None),
        account_type: Set(None),
        iban: Set(None),
        bic: Set(None),
        institute: Set(None),
        label: Set(Some(label.to_string())),
        currency: Set(currency.to_string()),
        raw_payload: Set(None),
        origin: Set("test".to_string()),
        first_seen_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert account");
    account_id
}

/// Links `user_id` to `account_id` by inserting a `user_account` row
/// directly — `crate::auth::link_account` resolves by `username` +
/// `source:external_id` (the `user-admin link` shape), but these tests
/// already hold both ids, so the direct insert is equivalent and simpler.
pub async fn link(db: &DatabaseConnection, user_id: Uuid, account_id: Uuid) {
    user_account::ActiveModel {
        user_id: Set(user_id),
        account_id: Set(account_id),
        created_at: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("link user to account");
}

/// Inserts one transaction row. `booking_date` is `YYYY-MM-DD`.
pub async fn seed_transaction(
    db: &DatabaseConnection,
    account_id: Uuid,
    booking_date: &str,
    amount: &str,
    counterparty_name: Option<&str>,
) -> Uuid {
    let tx_id = Uuid::new_v4();
    transaction::ActiveModel {
        id: Set(tx_id),
        account_id: Set(account_id),
        source: Set("test".to_string()),
        external_id: Set(tx_id.to_string()),
        booking_date: Set(chrono::NaiveDate::parse_from_str(booking_date, "%Y-%m-%d").unwrap()),
        valuta_date: Set(None),
        booking_status: Set("BOOKED".to_string()),
        amount: Set(amount.parse::<Decimal>().unwrap()),
        currency: Set("EUR".to_string()),
        counterparty_name: Set(counterparty_name.map(str::to_string)),
        counterparty_iban: Set(None),
        description: Set(None),
        transaction_type: Set(None),
        raw_payload: Set(serde_json::json!({})),
        origin: Set("test".to_string()),
        imported_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
        counterparty_key: Set(None),
    }
    .insert(db)
    .await
    .expect("insert transaction");
    tx_id
}
