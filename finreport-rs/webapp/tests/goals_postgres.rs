//! Iteration 4 WP-A integration tests: the `finreport.goal` projection
//! (project, tombstone, stale record, replay) and the contribution-row query
//! behind `evaluate_goal` / `goal_transaction_ids`, against a real Postgres.
#![cfg(feature = "integration")]

#[path = "support/mod.rs"]
mod support;

use chrono::{NaiveDate, TimeZone, Utc};
use entity::entities::{account, goal, transaction};
use rust_decimal::Decimal;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, EntityTrait, Set, Statement};
use std::str::FromStr;
use uuid::Uuid;
use webapp::goals::{evaluate_goal, goal_transaction_ids, ProgressWindow};
use webapp::kafka::envelope::Envelope;
use webapp::kafka::goals::{
    Cadence, Combine, GoalPeriod, GoalRecord, GoalScope, GoalType, PeriodKind, TOPIC_GOAL,
    CURRENT_SCHEMA_VERSION,
};
use webapp::projection::mapper::MapperRegistry;
use webapp::projection::{process_batch, ConsumedRecord};

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn envelope() -> Envelope {
    Envelope {
        source: "finreport".to_string(),
        source_account_id: None,
        origin: "user".to_string(),
        schema_version: 1,
        imported_at: Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap(),
        comdirect_account_key: None,
        comdirect_account_name: None,
    }
}

fn goal_record(id: Uuid, name: &str, minute: u32) -> GoalRecord {
    GoalRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        id,
        owner_user_id: Uuid::new_v4(),
        name: name.to_string(),
        goal_type: GoalType::SpendingLimit,
        amount: dec("200.0000"),
        currency: "EUR".to_string(),
        scope: GoalScope {
            category_slugs: vec!["leisure.hobbies".to_string()],
            tags: vec!["hobby".to_string()],
            combine: Combine::Any,
            tag_combine: Combine::All,
        },
        period: GoalPeriod {
            kind: PeriodKind::Recurring,
            cadence: Some(Cadence::Monthly),
            start_date: None,
            end_date: None,
        },
        archived: false,
        revision: Utc.with_ymd_and_hms(2027, 1, 1, 10, minute, 0).unwrap(),
    }
}

fn consumed(offset: i64, id: Uuid, record: Option<&GoalRecord>) -> ConsumedRecord {
    ConsumedRecord {
        topic: TOPIC_GOAL.to_string(),
        partition: 0,
        offset,
        key: Some(id.to_string()),
        payload: record
            .map(|r| serde_json::to_vec(r).unwrap())
            .unwrap_or_default(),
        envelope: envelope(),
    }
}

async fn apply(db: &DatabaseConnection, records: &[ConsumedRecord]) {
    process_batch(db, "default", &MapperRegistry::with_default_mappers(), None, records)
        .await
        .expect("goal batch applies");
}

async fn goal_rows(db: &DatabaseConnection) -> Vec<goal::Model> {
    let mut rows = goal::Entity::find().all(db).await.unwrap();
    rows.sort_by_key(|g| g.id);
    rows
}

#[tokio::test]
async fn a_goal_record_projects_every_field() {
    let pg = support::TestPostgres::start().await;
    let id = Uuid::new_v4();
    let record = goal_record(id, "Hobbies", 0);
    apply(pg.connection(), &[consumed(0, id, Some(&record))]).await;

    let row = goal::Entity::find_by_id(id).one(pg.connection()).await.unwrap().unwrap();
    assert_eq!(row.owner_user_id, record.owner_user_id);
    assert_eq!(row.name, "Hobbies");
    assert_eq!(row.goal_type, "spending_limit");
    assert_eq!(row.amount, dec("200.0000"));
    assert_eq!(row.scope_category_slugs, vec!["leisure.hobbies"]);
    assert_eq!(row.scope_tags, vec!["hobby"]);
    assert_eq!(row.scope_combine, "any");
    assert_eq!(row.scope_tag_combine, "all");
    assert_eq!(row.period_kind, "recurring");
    assert_eq!(row.period_cadence.as_deref(), Some("monthly"));
    assert_eq!(row.period_start, None);
    assert!(!row.archived);
}

#[tokio::test]
async fn a_newer_record_updates_and_a_stale_one_is_ignored() {
    let pg = support::TestPostgres::start().await;
    let id = Uuid::new_v4();
    apply(pg.connection(), &[consumed(0, id, Some(&goal_record(id, "v1", 5)))]).await;
    apply(pg.connection(), &[consumed(1, id, Some(&goal_record(id, "v2", 9)))]).await;
    apply(pg.connection(), &[consumed(2, id, Some(&goal_record(id, "stale", 1)))]).await;
    let row = goal::Entity::find_by_id(id).one(pg.connection()).await.unwrap().unwrap();
    assert_eq!(row.name, "v2");
}

#[tokio::test]
async fn a_tombstone_deletes_the_goal() {
    let pg = support::TestPostgres::start().await;
    let id = Uuid::new_v4();
    apply(pg.connection(), &[consumed(0, id, Some(&goal_record(id, "Hobbies", 0)))]).await;
    apply(pg.connection(), &[consumed(1, id, None)]).await;
    assert!(goal_rows(pg.connection()).await.is_empty());
}

#[tokio::test]
async fn replay_from_offset_zero_reproduces_the_table_identically() {
    let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let log = vec![
        consumed(0, a, Some(&goal_record(a, "a1", 1))),
        consumed(1, b, Some(&goal_record(b, "b1", 2))),
        consumed(2, a, Some(&goal_record(a, "a2", 3))),
        consumed(3, c, Some(&goal_record(c, "c1", 4))),
        consumed(4, b, None),
    ];

    let first = support::TestPostgres::start().await;
    apply(first.connection(), &log).await;
    let before = goal_rows(first.connection()).await;
    assert_eq!(before.len(), 2);

    // Same log again over the populated table (a redelivery) ...
    apply(first.connection(), &log).await;
    assert_eq!(goal_rows(first.connection()).await, before);

    // ... and from scratch on a fresh database (a reset + replay).
    let fresh = support::TestPostgres::start().await;
    apply(fresh.connection(), &log).await;
    assert_eq!(goal_rows(fresh.connection()).await, before);
}

// ---------------------------------------------------------------------------
// Contribution-row query
// ---------------------------------------------------------------------------

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn add_account(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    account::ActiveModel {
        id: Set(id),
        source: Set("comdirect".into()),
        external_id: Set(format!("ACC-{id}")),
        display_id: Set(None),
        account_type: Set(None),
        iban: Set(Some(format!("DE{id}"))),
        bic: Set(None),
        institute: Set(None),
        label: Set(None),
        currency: Set("EUR".into()),
        raw_payload: Set(None),
        origin: Set("comdirect".into()),
        first_seen_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn add_tx(db: &DatabaseConnection, account_id: Uuid, day: NaiveDate, amount: &str) -> Uuid {
    let id = Uuid::new_v4();
    transaction::ActiveModel {
        id: Set(id),
        account_id: Set(account_id),
        source: Set("comdirect".into()),
        external_id: Set(id.to_string()),
        booking_date: Set(day),
        valuta_date: Set(None),
        booking_status: Set("BOOKED".into()),
        amount: Set(dec(amount)),
        currency: Set("EUR".into()),
        counterparty_name: Set(None),
        counterparty_iban: Set(None),
        description: Set(None),
        transaction_type: Set(None),
        raw_payload: Set(serde_json::json!({})),
        origin: Set("comdirect".into()),
        imported_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
        counterparty_key: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn add_category(db: &DatabaseConnection, slug: &str, kind: &str) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO category (id, slug, name, kind, depth, sort_order, archived, origin, revision) \
         VALUES ($1, $2, $2, $3, 1, 0, false, 'seed', now())",
        vec![id.into(), slug.into(), kind.into()],
    )
    .await;
    id
}

async fn label(db: &DatabaseConnection, tx: Uuid, category: Option<Uuid>, status: &str) {
    exec(
        db,
        "INSERT INTO transaction_label (transaction_id, category_id, label_source, status, labeled_at) \
         VALUES ($1, $2, 'llm', $3, now())",
        vec![tx.into(), category.into(), status.into()],
    )
    .await;
}

async fn split(db: &DatabaseConnection, tx: Uuid, part: i32, amount: &str, category: Uuid) {
    exec(
        db,
        "INSERT INTO transaction_split (id, transaction_id, part_index, amount, category_id, invalid) \
         VALUES ($1, $2, $3, $4::numeric, $5, false)",
        vec![Uuid::new_v4().into(), tx.into(), part.into(), amount.into(), category.into()],
    )
    .await;
}

async fn tag(db: &DatabaseConnection, tx: Uuid, name: &str) {
    exec(
        db,
        "INSERT INTO transaction_tag (transaction_id, tag, revision) VALUES ($1, $2, now())",
        vec![tx.into(), name.into()],
    )
    .await;
}

async fn mark_transfer(db: &DatabaseConnection, tx: Uuid) {
    exec(
        db,
        "INSERT INTO transaction_insight (transaction_id, is_transfer, is_recurring, detected_at, revision) \
         VALUES ($1, true, false, now(), now())",
        vec![tx.into()],
    )
    .await;
}

fn monthly_food_goal() -> goal::Model {
    goal::Model {
        id: Uuid::new_v4(),
        owner_user_id: Uuid::new_v4(),
        name: "Food".into(),
        goal_type: "spending_limit".into(),
        amount: dec("100"),
        currency: "EUR".into(),
        scope_category_slugs: vec!["food".into()],
        scope_tags: vec![],
        scope_combine: "all".into(),
        scope_tag_combine: "all".into(),
        period_kind: "recurring".into(),
        period_cadence: Some("monthly".into()),
        period_start: None,
        period_end: None,
        archived: false,
        revision: Utc::now().into(),
    }
}

#[tokio::test]
async fn evaluation_applies_scope_splits_transfers_pending_and_account_scoping() {
    let pg = support::TestPostgres::start().await;
    let db = pg.connection();
    let acct = add_account(db).await;
    let other_acct = add_account(db).await;
    let food = add_category(db, "food", "expense").await;
    let groceries = add_category(db, "food.groceries", "expense").await;
    let leisure = add_category(db, "leisure", "expense").await;
    let day = date(2026, 9, 10);

    // Plain, in scope (a descendant).
    let plain = add_tx(db, acct, day, "-10.00").await;
    label(db, plain, Some(groceries), "resolved").await;
    // Refund inside the scope reduces the total.
    let refund = add_tx(db, acct, day, "3.00").await;
    label(db, refund, Some(food), "resolved").await;
    // Split: -50 as -30 food + -20 leisure; counted by parts, once.
    let split_tx = add_tx(db, acct, day, "-50.00").await;
    label(db, split_tx, None, "resolved").await;
    split(db, split_tx, 0, "-30.00", food).await;
    split(db, split_tx, 1, "-20.00", leisure).await;
    // Internal transfer in scope never counts.
    let transfer = add_tx(db, acct, day, "-99.00").await;
    label(db, transfer, Some(food), "resolved").await;
    mark_transfer(db, transfer).await;
    // Held for review: pending, not total.
    let held = add_tx(db, acct, day, "-7.00").await;
    label(db, held, Some(food), "needs_review").await;
    // Uncategorised (NULL category on a label row): pending for a category scope.
    let uncat = add_tx(db, acct, day, "-2.00").await;
    label(db, uncat, None, "resolved").await;
    // Out of scope, and an account the caller does not own.
    let rent = add_tx(db, acct, day, "-500.00").await;
    label(db, rent, Some(leisure), "resolved").await;
    let foreign = add_tx(db, other_acct, day, "-1000.00").await;
    label(db, foreign, Some(food), "resolved").await;

    let g = monthly_food_goal();
    let progress = evaluate_goal(
        db,
        &g,
        &[acct],
        Some(ProgressWindow { start: date(2026, 9, 1), end: date(2026, 9, 30) }),
    )
    .await
    .unwrap();

    assert_eq!(progress.buckets.len(), 1);
    assert_eq!(progress.buckets[0].label, "Sep 2026");
    // 10 - 3 + 30
    assert_eq!(progress.total, dec("37.00"));
    // 7 held + 2 uncategorised
    assert_eq!(progress.pending, dec("9.00"));

    let ids = goal_transaction_ids(db, &g, &[acct], date(2026, 9, 1), date(2026, 9, 30))
        .await
        .unwrap();
    let mut expected = vec![plain, refund, split_tx, held, uncat];
    expected.sort();
    let mut got = ids.clone();
    got.sort();
    assert_eq!(got, expected);
    assert_eq!(ids.len(), 5, "the split is listed once");
}

#[tokio::test]
async fn saving_target_counts_only_saving_kind_and_tag_scope_ignores_categories() {
    let pg = support::TestPostgres::start().await;
    let db = pg.connection();
    let acct = add_account(db).await;
    let etf = add_category(db, "savings.etf", "saving").await;
    let food = add_category(db, "food", "expense").await;
    let day = date(2026, 9, 10);

    let into_etf = add_tx(db, acct, day, "-250.00").await;
    label(db, into_etf, Some(etf), "resolved").await;
    let food_tx = add_tx(db, acct, day, "-40.00").await;
    label(db, food_tx, Some(food), "resolved").await;
    tag(db, food_tx, "goal").await;
    tag(db, into_etf, "goal").await;

    let mut g = monthly_food_goal();
    g.goal_type = "saving_target".into();
    g.scope_category_slugs = vec![];
    g.scope_tags = vec!["goal".into()];
    let w = Some(ProgressWindow { start: date(2026, 9, 1), end: date(2026, 9, 30) });
    let p = evaluate_goal(db, &g, &[acct], w).await.unwrap();
    assert_eq!(p.total, dec("250.00"));
    assert_eq!(p.pending, dec("0"));
}
