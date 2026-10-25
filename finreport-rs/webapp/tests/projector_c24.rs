//! C24 records through the projector: the rows land in `account` /
//! `transaction` with `source = "c24"`, and re-projecting the same records
//! (or an overlapping export) changes nothing - the property the whole
//! `external_id` identity scheme exists for.
//!
//! Synthetic data only. Gated behind the `integration` feature; uses the
//! shared throwaway-Postgres harness.
#![cfg(feature = "integration")]

#[path = "support/mod.rs"]
mod support;

use c24_csv::{C24Row, HEADER};
use chrono::{TimeZone, Utc};
use entity::entities::{account, transaction};
use rust_decimal::Decimal;
use sea_orm::{EntityTrait, QueryOrder};
use webapp::kafka::c24::{
    C24AccountRecord, C24TransactionRecord, SOURCE_C24, TOPIC_C24_ACCOUNT, TOPIC_C24_TRANSACTION,
};
use webapp::kafka::envelope::{account_uuid, transaction_uuid, Envelope};
use webapp::projection::mapper::MapperRegistry;
use webapp::projection::{process_batch, ConsumedRecord};

const GROUP: &str = "default";
const ACCOUNT_NUMBER: &str = "0000012345";

/// Two byte-identical card rows (genuinely distinct transactions) plus one
/// transfer, all on one account.
fn export(rows: &[&str]) -> Vec<C24Row> {
    let mut csv = HEADER.join(",");
    for row in rows {
        csv.push('\n');
        csv.push_str(row);
    }
    c24_csv::parse(csv.as_bytes()).expect("synthetic export parses")
}

const CARD: &str = r#"Kartenzahlung,04.03.2026,03.03.2026 18:42,"-1,00 €",Beispiel Baeckerei,,,,,0000012345,Testkonto,Essen,Baeckerei"#;
const TRANSFER: &str = r#"Echtzeit-Überweisung,05.03.2026,,"1.500,00 €",Max Mustermann,DE00000000000000000000,TESTDE00XXX,Miete Maerz,,0000012345,Testkonto,,"#;

fn envelope(source_account_id: Option<&str>) -> Envelope {
    Envelope {
        source: SOURCE_C24.to_string(),
        source_account_id: source_account_id.map(str::to_string),
        origin: "source".to_string(),
        schema_version: 1,
        imported_at: Utc.with_ymd_and_hms(2026, 3, 6, 9, 0, 0).unwrap(),
        comdirect_account_key: None,
        comdirect_account_name: None,
    }
}

fn account_record(offset: i64, row: &C24Row) -> ConsumedRecord {
    let rec = C24AccountRecord::from_row(row);
    ConsumedRecord {
        topic: TOPIC_C24_ACCOUNT.to_string(),
        partition: 0,
        offset,
        key: Some(rec.topic_key().to_string()),
        payload: serde_json::to_vec(&rec).unwrap(),
        envelope: envelope(None),
    }
}

fn transaction_record(offset: i64, row: &C24Row) -> ConsumedRecord {
    let rec = C24TransactionRecord::from_row(row);
    ConsumedRecord {
        topic: TOPIC_C24_TRANSACTION.to_string(),
        partition: 0,
        offset,
        key: Some(rec.topic_key().to_string()),
        payload: serde_json::to_vec(&rec).unwrap(),
        envelope: envelope(Some(&row.account_number)),
    }
}

fn records(rows: &[C24Row]) -> Vec<ConsumedRecord> {
    let mut out = vec![account_record(0, &rows[0])];
    for (i, row) in rows.iter().enumerate() {
        out.push(transaction_record(i as i64, row));
    }
    out
}

async fn tx_rows(db: &sea_orm::DatabaseConnection) -> Vec<transaction::Model> {
    transaction::Entity::find()
        .order_by_asc(transaction::Column::Id)
        .all(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn c24_records_land_as_c24_rows_and_reprojecting_changes_nothing() {
    let db = support::TestPostgres::start().await;
    let registry = MapperRegistry::with_default_mappers();
    let rows = export(&[CARD, CARD, TRANSFER]);
    assert_eq!(rows.len(), 3);
    let batch = records(&rows);

    let stats = process_batch(db.connection(), GROUP, &registry, None, &batch)
        .await
        .unwrap();
    assert_eq!((stats.applied, stats.skipped), (4, 0));

    let accounts = account::Entity::find().all(db.connection()).await.unwrap();
    assert_eq!(accounts.len(), 1);
    let acct = &accounts[0];
    assert_eq!(acct.source, "c24");
    assert_eq!(acct.external_id, ACCOUNT_NUMBER);
    assert_eq!(acct.id, account_uuid("c24", ACCOUNT_NUMBER));
    assert_eq!(acct.label.as_deref(), Some("Testkonto"));
    assert_eq!(acct.iban, None);
    assert_eq!(acct.origin, "source");

    let first = tx_rows(db.connection()).await;
    assert_eq!(first.len(), 3, "the identical card rows stay distinct");
    assert!(first.iter().all(|t| t.source == "c24" && t.account_id == acct.id));
    let transfer = first
        .iter()
        .find(|t| t.transaction_type.as_deref() == Some("Echtzeit-Überweisung"))
        .unwrap();
    assert_eq!(transfer.id, transaction_uuid("c24", &transfer.external_id));
    assert_eq!(transfer.amount, Decimal::new(150000, 2));
    assert_eq!(transfer.description.as_deref(), Some("Miete Maerz"));
    assert_eq!(transfer.booking_status, "BOOKED");

    // Re-project the identical records: every row is byte-for-byte the same.
    process_batch(db.connection(), GROUP, &registry, None, &batch)
        .await
        .unwrap();
    assert_eq!(tx_rows(db.connection()).await, first);
    assert_eq!(
        account::Entity::find().all(db.connection()).await.unwrap(),
        accounts
    );

    // An overlapping, later export (one row repeated, ordinals unchanged)
    // adds no row either.
    let overlap = export(&[CARD, CARD]);
    process_batch(db.connection(), GROUP, &registry, None, &records(&overlap))
        .await
        .unwrap();
    assert_eq!(tx_rows(db.connection()).await.len(), 3);
}

#[tokio::test]
async fn transaction_before_account_is_stubbed_then_filled_in() {
    let db = support::TestPostgres::start().await;
    let registry = MapperRegistry::with_default_mappers();
    let rows = export(&[TRANSFER]);

    process_batch(
        db.connection(),
        GROUP,
        &registry,
        None,
        &[transaction_record(0, &rows[0])],
    )
    .await
    .unwrap();
    let stub = account::Entity::find().one(db.connection()).await.unwrap().unwrap();
    assert_eq!(stub.origin, "stub");

    process_batch(
        db.connection(),
        GROUP,
        &registry,
        None,
        &[account_record(0, &rows[0])],
    )
    .await
    .unwrap();
    let filled = account::Entity::find().one(db.connection()).await.unwrap().unwrap();
    assert_eq!(filled.origin, "source");
    assert_eq!(filled.label.as_deref(), Some("Testkonto"));
    assert_eq!(tx_rows(db.connection()).await.len(), 1);
}
