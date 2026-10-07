//! Shared seeding/connection helpers for the `integration`-gated tests
//! (§8/§9). Connects to the throwaway `finreport-wp4-pg` container
//! (`docker run --name finreport-wp4-pg -p 55435:5432 ...` — see the WP4
//! final report for the exact invocation) rather than spinning up
//! `testcontainers` per test, per the task's explicit instruction. Every
//! seed id is a fresh `Uuid::new_v4()`, so tests sharing one database stay
//! independent without needing per-test truncation.

#![allow(dead_code)]

use chrono::Utc;
use entity::entities::{account, app_user, category, transaction, transaction_label, user_account};
use rust_decimal::Decimal;
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, Set};
use secrecy::SecretString;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;
use webapp::auth;

#[path = "../support/migrate.rs"]
mod migrate;

const TEST_DATABASE_URL: &str = "postgres://postgres:postgres@127.0.0.1:55435/finreport_wp4";

/// Connects fresh per test (rather than sharing one pool via a `static`):
/// `sqlx`'s pool ties its background tasks to the Tokio runtime it was
/// created on, and each `#[tokio::test]` spins up its own runtime, so a
/// shared pool intermittently hits `ConnectionAcquire(Timeout)` once an
/// earlier test's runtime has shut down.
///
/// Migrations run through [`migrate::run_migrations_once`] (§9.2), not
/// `webapp::db::seaql::init_db` — every test in this binary targets the
/// same `finreport-wp4-pg` container, and `init_db` ran `Migrator::up`
/// unconditionally on every call, which raced `sea-orm-migration`'s tracking
/// table the moment two `#[tokio::test]`s ran concurrently. Memoizing by URL
/// means only the first caller actually migrates; everyone else awaits that
/// result.
pub async fn db() -> Arc<DatabaseConnection> {
    let connection = Database::connect(TEST_DATABASE_URL)
        .await
        .expect("connect to finreport-wp4-pg on 127.0.0.1:55435 — is the container up?");
    migrate::run_migrations_once(TEST_DATABASE_URL, &connection)
        .await
        .expect("run migrations against finreport-wp4-pg");
    Arc::new(connection)
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
        admin_username: "admin".to_string(),
        admin_password: None,
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
        is_admin: Set(false),
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

/// Inserts one root-level (no `parent_id`) `category` row with
/// `depth = 1`, `origin = "seed"`, un-archived. `slug_prefix` is suffixed
/// with a fresh random id (like [`seed_user`]'s username) so reruns against
/// a not-yet-cleaned-up database don't collide on `category.slug`'s unique
/// index; returns the actual stored slug alongside the id.
pub async fn seed_category(
    db: &DatabaseConnection,
    slug_prefix: &str,
    name: &str,
    kind: &str,
) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let slug = format!("{slug_prefix}_{}", id.simple());
    category::ActiveModel {
        id: Set(id),
        slug: Set(slug.clone()),
        parent_id: Set(None),
        name: Set(name.to_string()),
        kind: Set(kind.to_string()),
        depth: Set(1),
        sort_order: Set(0),
        archived: Set(false),
        origin: Set("seed".to_string()),
        owner_user_id: Set(None),
        revision: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert category");
    (id, slug)
}

/// Inserts a `transaction_label` row directly, as if WP3's projector had
/// already applied a `transaction-label` event for it (§3) — the WP3
/// projections this module reads from aren't implemented on this branch,
/// so tests seed them directly per the task's instructions.
pub async fn seed_transaction_label(
    db: &DatabaseConnection,
    transaction_id: Uuid,
    category_id: Option<Uuid>,
    label_source: &str,
    status: &str,
) -> Uuid {
    transaction_label::ActiveModel {
        transaction_id: Set(transaction_id),
        category_id: Set(category_id),
        label_source: Set(label_source.to_string()),
        rule_id: Set(None),
        confidence: Set(None),
        status: Set(status.to_string()),
        review_reason: Set(None),
        proposed_category_path: Set(None),
        provider: Set(None),
        model: Set(None),
        prompt_version: Set(None),
        fingerprint: Set(None),
        reasoning: Set(None),
        labeled_at: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert transaction_label");
    transaction_id
}
