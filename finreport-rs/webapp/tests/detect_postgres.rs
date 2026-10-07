//! WP-A detection integration tests (docs/specs/iteration-3.md §7 "WP-A |
//! integration"): drives `detect::processor::run_detection_pass` directly
//! against a real Postgres + Kafka pair (the shared
//! `support::TestPostgres`/`TestKafka` harness, same shape as
//! `labeler_postgres.rs`).
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

/// Inserts the `app_user` row a `user_account` link's foreign key requires.
async fn seed_user(db: &sea_orm::DatabaseConnection, user_id: Uuid) {
    app_user::ActiveModel {
        id: Set(user_id),
        username: Set(format!("test-user-{user_id}")),
        password_hash: Set("unused".to_string()),
        display_name: Set(None),
        disabled: Set(false),
        created_at: Set(Utc::now().into()),
        is_admin: Set(false),
    }
    .insert(db)
    .await
    .expect("insert test app_user");
}

/// Inserts an `account` row owned by `owner` (if given).
async fn seed_account(
    db: &sea_orm::DatabaseConnection,
    external_id: &str,
    iban: Option<&str>,
    owner: Option<Uuid>,
) -> Uuid {
    let account_id = webapp::kafka::envelope::account_uuid("comdirect", external_id);
    account::ActiveModel {
        id: Set(account_id),
        source: Set("comdirect".to_string()),
        external_id: Set(external_id.to_string()),
        display_id: Set(None),
        account_type: Set(Some("Girokonto".to_string())),
        iban: Set(iban.map(str::to_string)),
        bic: Set(None),
        institute: Set(None),
        label: Set(None),
        currency: Set("EUR".to_string()),
        raw_payload: Set(None),
        origin: Set("comdirect".to_string()),
        first_seen_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("insert test account");

    if let Some(user_id) = owner {
        user_account::ActiveModel {
            user_id: Set(user_id),
            account_id: Set(account_id),
            created_at: Set(Utc::now().into()),
        }
        .insert(db)
        .await
        .expect("insert user_account");
    }

    account_id
}

#[allow(clippy::too_many_arguments)]
async fn seed_transaction(
    db: &sea_orm::DatabaseConnection,
    account_id: Uuid,
    external_id: &str,
    booking_date: chrono::NaiveDate,
    amount: Decimal,
    counterparty_iban: Option<&str>,
    counterparty_key: Option<&str>,
) -> Uuid {
    let txn_id = webapp::kafka::envelope::transaction_uuid("comdirect", external_id);
    transaction::ActiveModel {
        id: Set(txn_id),
        account_id: Set(account_id),
        source: Set("comdirect".to_string()),
        external_id: Set(external_id.to_string()),
        booking_date: Set(booking_date),
        valuta_date: Set(Some(booking_date)),
        booking_status: Set("BOOKED".to_string()),
        amount: Set(amount),
        currency: Set("EUR".to_string()),
        counterparty_name: Set(Some("Test Counterparty".to_string())),
        counterparty_iban: Set(counterparty_iban.map(str::to_string)),
        description: Set(None),
        transaction_type: Set(None),
        raw_payload: Set(serde_json::json!({})),
        origin: Set("comdirect".to_string()),
        imported_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
        counterparty_key: Set(counterparty_key.map(str::to_string)),
    }
    .insert(db)
    .await
    .expect("insert test transaction");
    txn_id
}

/// `today` minus `months_ago` whole months, same day-of-month — keeps fixture
/// dates inside the (relative-to-now) recurring detection window regardless
/// of when the test actually runs.
fn months_ago(months_ago: u32) -> chrono::NaiveDate {
    Utc::now().date_naive() - chrono::Months::new(months_ago)
}

#[tokio::test]
async fn fixture_replay_flags_a_transfer_pair_and_a_recurring_series() {
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

    let user = Uuid::new_v4();
    seed_user(&db, user).await;
    let acc_out = seed_account(&db, "ACC-OUT", Some("DE_OUT"), Some(user)).await;
    let acc_in = seed_account(&db, "ACC-IN", Some("DE_IN"), Some(user)).await;

    let txn_out = seed_transaction(
        &db,
        acc_out,
        "TRANSFER-OUT-01",
        months_ago(1),
        amt("-300.00"),
        Some("DE_IN"),
        None,
    )
    .await;
    let txn_in = seed_transaction(
        &db,
        acc_in,
        "TRANSFER-IN-01",
        months_ago(1),
        amt("300.00"),
        None,
        None,
    )
    .await;

    let acc_rec = seed_account(&db, "ACC-REC", None, Some(user)).await;
    let mut series_members = Vec::new();
    for (i, months) in [4u32, 3, 2, 1].into_iter().enumerate() {
        let id = seed_transaction(
            &db,
            acc_rec,
            &format!("RECURRING-{i}"),
            months_ago(months),
            amt("-39.90"),
            None,
            Some("gym-powergym"),
        )
        .await;
        series_members.push(id);
    }

    run_detection_pass(&db, &publisher)
        .await
        .expect("detection pass should succeed");

    let out_insight = transaction_insight::Entity::find_by_id(txn_out)
        .one(&db)
        .await
        .expect("query insight")
        .expect("outgoing leg must have an insight row");
    assert!(out_insight.is_transfer);
    assert_eq!(out_insight.transfer_counterpart_id, Some(txn_in));
    assert_eq!(out_insight.transfer_match.as_deref(), Some("iban"));

    let in_insight = transaction_insight::Entity::find_by_id(txn_in)
        .one(&db)
        .await
        .expect("query insight")
        .expect("incoming leg must have an insight row");
    assert!(in_insight.is_transfer);
    assert_eq!(in_insight.transfer_counterpart_id, Some(txn_out));

    let mut series_ids = std::collections::HashSet::new();
    for member in &series_members {
        let insight = transaction_insight::Entity::find_by_id(*member)
            .one(&db)
            .await
            .expect("query insight")
            .expect("series member must have an insight row");
        assert!(insight.is_recurring);
        series_ids.insert(insight.recurring_series_id.expect("series id set"));
    }
    assert_eq!(series_ids.len(), 1, "all 4 occurrences must share one series id");

    // §7 "a second pass publishes zero records": re-running must not change
    // any stored row's revision.
    let before = transaction_insight::Entity::find_by_id(txn_out)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .revision;
    run_detection_pass(&db, &publisher)
        .await
        .expect("second pass should also succeed");
    let after = transaction_insight::Entity::find_by_id(txn_out)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .revision;
    assert_eq!(before, after, "an unchanged pass must not republish");
}

#[tokio::test]
async fn a_pair_that_stops_matching_gets_un_flagged() {
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

    let user = Uuid::new_v4();
    seed_user(&db, user).await;
    let acc_a = seed_account(&db, "ACC-A", None, Some(user)).await;
    let acc_b = seed_account(&db, "ACC-B", None, Some(user)).await;

    let txn_a = seed_transaction(&db, acc_a, "PAIR-A", months_ago(1), amt("-50.00"), None, None).await;
    let txn_b = seed_transaction(&db, acc_b, "PAIR-B", months_ago(1), amt("50.00"), None, None).await;

    run_detection_pass(&db, &publisher).await.expect("first pass");
    let insight = transaction_insight::Entity::find_by_id(txn_a).one(&db).await.unwrap().unwrap();
    assert!(insight.is_transfer);

    // The pair stops matching: re-publish `txn_b` with a different amount
    // (simulating a correction), directly via the entity so the next pass
    // sees it changed.
    let mut active: transaction::ActiveModel = transaction::Entity::find_by_id(txn_b)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into();
    active.amount = Set(amt("75.00"));
    active.update(&db).await.expect("update transaction amount");

    run_detection_pass(&db, &publisher).await.expect("second pass");

    let insight_a = transaction_insight::Entity::find_by_id(txn_a).one(&db).await.unwrap().unwrap();
    assert!(!insight_a.is_transfer, "un-flagging must publish when a pair stops matching");
    let insight_b = transaction_insight::Entity::find_by_id(txn_b).one(&db).await.unwrap().unwrap();
    assert!(!insight_b.is_transfer);
}

#[tokio::test]
async fn a_tombstone_deletes_the_projected_row() {
    let pg = TestPostgres::start().await;
    let db = webapp::db::seaql::init_db(pg.database_url())
        .await
        .expect("connect to test Postgres");

    let transaction_id = Uuid::new_v4();
    let record = webapp::kafka::insights::InsightRecord {
        schema_version: webapp::kafka::insights::CURRENT_SCHEMA_VERSION,
        source: "comdirect".to_string(),
        external_id: "TOMBSTONE-01".to_string(),
        is_transfer: true,
        transfer_counterpart: None,
        transfer_match: Some(webapp::kafka::insights::TransferMatch::AmountDate),
        is_recurring: false,
        recurring_series_id: None,
        recurring_cadence: None,
        recurring_median_amount: None,
        detected_at: Utc::now(),
        revision: Utc::now(),
    };
    webapp::projection::insights::project_insight(&db, transaction_id, Some(record))
        .await
        .expect("insert insight row");
    assert!(transaction_insight::Entity::find_by_id(transaction_id)
        .one(&db)
        .await
        .unwrap()
        .is_some());

    webapp::projection::insights::project_insight(&db, transaction_id, None)
        .await
        .expect("tombstone the insight row");
    assert!(transaction_insight::Entity::find_by_id(transaction_id)
        .one(&db)
        .await
        .unwrap()
        .is_none());
}
