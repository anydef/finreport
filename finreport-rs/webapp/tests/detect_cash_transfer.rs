//! Does the transfer detector pair a **cash** leg with a bank leg?
//!
//! The user's third cash-account requirement was: *"I should be able to link
//! 'tranfer' transactions from my normal account to my cash account."* The
//! plan answers it by **reusing the existing detector** rather than adding a
//! new concept — `docs/TODO.md` prefers that over widening
//! `transaction_link`, whose `kind` is deliberately only `REIMBURSEMENT`.
//!
//! That plan rests on three properties of code written before cash accounts
//! existed, none of which was ever asserted:
//!
//! 1. `detect::transfer`'s four hard rules do **not** filter on `source`, so
//!    a `source = "cash"` row is an eligible leg.
//! 2. `load_txn_facts` selects every `transaction` and every `account` with
//!    no source filter, and takes owners from `user_account` — which the cash
//!    projection populates from the record's own `user_id`.
//! 3. A cash row has no IBAN, so the pair can only be `AmountDate`, never
//!    `Iban`.
//!
//! Reading the code says all three hold. This asserts them, because "the
//! feature works by reusing X" is the kind of claim that is cheap to believe
//! and expensive to be wrong about: if the detector silently skipped cash
//! rows, the user would enter an ATM withdrawal, see no pairing, and the
//! cashflow would double-count the money as both spending and cash income.
#![cfg(feature = "integration")]

#[path = "support/mod.rs"]
mod support;

use chrono::Utc;
use entity::entities::{account, app_user, transaction, transaction_insight, user_account};
use rust_decimal::Decimal;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};
use std::str::FromStr;
use support::{TestKafka, TestPostgres};
use uuid::Uuid;
use webapp::detect::processor::run_detection_pass;
use webapp::kafka::producer::EventPublisher;

fn amt(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn seed_user(db: &sea_orm::DatabaseConnection) -> Uuid {
    let user_id = Uuid::new_v4();
    app_user::ActiveModel {
        id: Set(user_id),
        username: Set(format!("cash-test-{user_id}")),
        password_hash: Set("unused".to_string()),
        display_name: Set(None),
        disabled: Set(false),
        created_at: Set(Utc::now().into()),
        is_admin: Set(false),
    }
    .insert(db)
    .await
    .expect("insert app_user");
    user_id
}

/// An account of an arbitrary `source`, so the bank and cash sides are seeded
/// by the same helper and the only difference is the data under test.
async fn seed_account(
    db: &sea_orm::DatabaseConnection,
    source: &str,
    external_id: &str,
    iban: Option<&str>,
    owner: Uuid,
) -> Uuid {
    let account_id = webapp::kafka::envelope::account_uuid(source, external_id);
    account::ActiveModel {
        id: Set(account_id),
        source: Set(source.to_string()),
        external_id: Set(external_id.to_string()),
        display_id: Set(None),
        account_type: Set(None),
        iban: Set(iban.map(str::to_string)),
        bic: Set(None),
        institute: Set(None),
        label: Set(Some(format!("{source} account"))),
        currency: Set("EUR".to_string()),
        raw_payload: Set(None),
        // A cash account is user-authored, a bank account is imported.
        origin: Set(if source == "cash" { "user" } else { "source" }.to_string()),
        first_seen_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert account");

    user_account::ActiveModel {
        user_id: Set(owner),
        account_id: Set(account_id),
        created_at: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert user_account");

    account_id
}

#[allow(clippy::too_many_arguments)]
async fn seed_transaction(
    db: &sea_orm::DatabaseConnection,
    source: &str,
    account_id: Uuid,
    external_id: &str,
    booking_date: chrono::NaiveDate,
    amount: Decimal,
    counterparty_iban: Option<&str>,
) -> Uuid {
    let txn_id = webapp::kafka::envelope::transaction_uuid(source, external_id);
    transaction::ActiveModel {
        id: Set(txn_id),
        account_id: Set(account_id),
        source: Set(source.to_string()),
        external_id: Set(external_id.to_string()),
        booking_date: Set(booking_date),
        valuta_date: Set(None),
        booking_status: Set("BOOKED".to_string()),
        amount: Set(amount),
        currency: Set("EUR".to_string()),
        counterparty_name: Set(Some("Cash withdrawal".to_string())),
        counterparty_iban: Set(counterparty_iban.map(str::to_string)),
        description: Set(None),
        transaction_type: Set(None),
        raw_payload: Set(serde_json::json!({})),
        origin: Set(if source == "cash" { "user" } else { "source" }.to_string()),
        imported_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
        counterparty_key: Set(None),
    }
    .insert(db)
    .await
    .expect("insert transaction");
    txn_id
}

async fn insight(
    db: &sea_orm::DatabaseConnection,
    id: Uuid,
) -> Option<transaction_insight::Model> {
    transaction_insight::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("query transaction_insight")
}

/// The ATM case, which is what the user described: money leaves the bank
/// account and the same amount is entered by hand on the cash account.
#[tokio::test]
async fn an_atm_withdrawal_pairs_the_bank_leg_with_the_hand_entered_cash_leg() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url())
        .await
        .expect("connect to test Postgres");
    let publisher =
        EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    let user = seed_user(&db).await;
    let bank = seed_account(&db, "comdirect", "CASH-BANK-1", Some("DE_BANK_1"), user).await;
    let wallet = seed_account(&db, "cash", "CASH-WALLET-1", None, user).await;

    let day = Utc::now().date_naive() - chrono::Duration::days(3);
    // The bank's own line for the withdrawal: an ATM gives no counterparty
    // IBAN, which is exactly why this can only ever be an `AmountDate` match.
    let bank_leg =
        seed_transaction(&db, "comdirect", bank, "ATM-OUT-1", day, amt("-150.00"), None).await;
    let cash_leg =
        seed_transaction(&db, "cash", wallet, "WALLET-IN-1", day, amt("150.00"), None).await;

    run_detection_pass(&db, &publisher)
        .await
        .expect("detection pass");

    let bank_insight = insight(&db, bank_leg).await.expect("bank leg has an insight");
    let cash_insight = insight(&db, cash_leg).await.expect("cash leg has an insight");

    // Both sides flagged, pointing at each other — the detector flags both,
    // which the savings and cashflow code depends on.
    assert!(bank_insight.is_transfer, "bank leg not flagged as a transfer");
    assert!(cash_insight.is_transfer, "cash leg not flagged as a transfer");
    assert_eq!(bank_insight.transfer_counterpart_id, Some(cash_leg));
    assert_eq!(cash_insight.transfer_counterpart_id, Some(bank_leg));

    // No IBAN on either side of this pair, so the rank bucket is amount+date.
    assert_eq!(bank_insight.transfer_match.as_deref(), Some("amount_date"));
    assert_eq!(cash_insight.transfer_match.as_deref(), Some("amount_date"));
}

/// Rule 2 is "at least one shared owner", and a cash account is linked to its
/// owner by the projection rather than by an importer — so this is the rule
/// most likely to break for cash specifically. An unlinked wallet must not
/// pair, or one user's hand-entered row could attach to another's bank line.
#[tokio::test]
async fn a_cash_account_owned_by_someone_else_does_not_pair() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let pg = TestPostgres::start().await;
    let broker = TestKafka::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url())
        .await
        .expect("connect to test Postgres");
    let publisher =
        EventPublisher::connect(broker.bootstrap_servers()).expect("connect test Kafka producer");

    let owner = seed_user(&db).await;
    let stranger = seed_user(&db).await;
    let bank = seed_account(&db, "comdirect", "CASH-BANK-2", Some("DE_BANK_2"), owner).await;
    let wallet = seed_account(&db, "cash", "CASH-WALLET-2", None, stranger).await;

    let day = Utc::now().date_naive() - chrono::Duration::days(2);
    let bank_leg =
        seed_transaction(&db, "comdirect", bank, "ATM-OUT-2", day, amt("-80.00"), None).await;
    let cash_leg =
        seed_transaction(&db, "cash", wallet, "WALLET-IN-2", day, amt("80.00"), None).await;

    run_detection_pass(&db, &publisher)
        .await
        .expect("detection pass");

    for (id, name) in [(bank_leg, "bank leg"), (cash_leg, "cash leg")] {
        let flagged = insight(&db, id)
            .await
            .is_some_and(|row| row.is_transfer);
        assert!(!flagged, "{name} was paired across two different owners");
    }
}
