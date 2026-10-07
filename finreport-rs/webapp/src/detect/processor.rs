//! Loader/publisher glue for [`super::detect_transfers`] and
//! [`super::detect_recurring`] (iteration 3 §3): reads `TxnFacts` with one
//! query joining `transaction`, `account`, `user_account`, runs both pure
//! algorithms, and publishes `transaction_insight` records that differ from
//! the projected row (compare-before-publish, same shape as the rule
//! learner, §2.3 "a second pass over an unchanged read model publishes
//! nothing").

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use entity::entities::{account, transaction, transaction_insight, user_account};
use rdkafka::message::{Header, OwnedHeaders};
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use tracing::error;
use utils::settings::Settings;
use uuid::Uuid;

use super::{
    detect_recurring, detect_transfers, Cadence, RecurringConfig, TransferConfig,
    TransferMatchKind, TxnFacts,
};
use crate::kafka::envelope::HEADER_ORIGIN;
use crate::kafka::insights::{
    InsightRecord, RecurringCadence as WireCadence, TransferMatch as WireMatch, ORIGIN_DETECTOR,
    TOPIC_TRANSACTION_INSIGHT,
};
use crate::kafka::producer::EventPublisher;
use crate::projection::insights::project_insight;

fn headers() -> OwnedHeaders {
    OwnedHeaders::new().insert(Header {
        key: HEADER_ORIGIN,
        value: Some(ORIGIN_DETECTOR),
    })
}

fn wire_match(kind: TransferMatchKind) -> WireMatch {
    match kind {
        TransferMatchKind::Iban => WireMatch::Iban,
        TransferMatchKind::AmountDate => WireMatch::AmountDate,
    }
}

fn wire_cadence(cadence: Cadence) -> WireCadence {
    match cadence {
        Cadence::Monthly => WireCadence::Monthly,
        Cadence::Quarterly => WireCadence::Quarterly,
        Cadence::Yearly => WireCadence::Yearly,
    }
}

/// Loads every transaction's detection-relevant facts with the §3 join
/// (`transaction` + `account` + `user_account`).
async fn load_txn_facts(db: &impl ConnectionTrait) -> Result<Vec<TxnFacts>, DbErr> {
    let transactions = transaction::Entity::find().all(db).await?;
    let accounts: HashMap<Uuid, account::Model> = account::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|a| (a.id, a))
        .collect();
    let mut owners: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for ua in user_account::Entity::find().all(db).await? {
        owners.entry(ua.account_id).or_default().push(ua.user_id);
    }

    Ok(transactions
        .into_iter()
        .map(|t| TxnFacts {
            id: t.id,
            account_id: t.account_id,
            source: t.source.clone(),
            external_id: t.external_id.clone(),
            owner_user_ids: owners.get(&t.account_id).cloned().unwrap_or_default(),
            booking_date: t.booking_date,
            amount: t.amount,
            counterparty_iban: t.counterparty_iban.clone(),
            account_iban: accounts.get(&t.account_id).and_then(|a| a.iban.clone()),
            counterparty_key: t.counterparty_key.clone(),
        })
        .collect())
}

/// Builds the desired `InsightRecord` for every transaction that should
/// have one: a transfer leg, a recurring-series member, or both. A
/// transaction with neither flag never gets an entry here — un-flagging a
/// previously-flagged row is [`run_detection_pass`]'s job, not this one's.
fn build_desired_records(
    txns: &[TxnFacts],
    transfer_cfg: TransferConfig,
    recurring_cfg: RecurringConfig,
    now: chrono::DateTime<Utc>,
) -> HashMap<Uuid, InsightRecord> {
    let by_id: HashMap<Uuid, &TxnFacts> = txns.iter().map(|t| (t.id, t)).collect();
    let mut desired: HashMap<Uuid, InsightRecord> = HashMap::new();

    let blank = |t: &TxnFacts| InsightRecord {
        schema_version: crate::kafka::insights::CURRENT_SCHEMA_VERSION,
        source: t.source.clone(),
        external_id: t.external_id.clone(),
        is_transfer: false,
        transfer_counterpart: None,
        transfer_match: None,
        is_recurring: false,
        recurring_series_id: None,
        recurring_cadence: None,
        recurring_median_amount: None,
        detected_at: now,
        revision: now,
    };

    for pair in detect_transfers(txns, transfer_cfg) {
        let Some(a) = by_id.get(&pair.a) else { continue };
        let Some(b) = by_id.get(&pair.b) else { continue };

        let rec_a = desired.entry(pair.a).or_insert_with(|| blank(a));
        rec_a.is_transfer = true;
        rec_a.transfer_counterpart = Some(format!("{}:{}", b.source, b.external_id));
        rec_a.transfer_match = Some(wire_match(pair.match_kind));

        let rec_b = desired.entry(pair.b).or_insert_with(|| blank(b));
        rec_b.is_transfer = true;
        rec_b.transfer_counterpart = Some(format!("{}:{}", a.source, a.external_id));
        rec_b.transfer_match = Some(wire_match(pair.match_kind));
    }

    let today = now.date_naive();
    for series in detect_recurring(txns, recurring_cfg, today) {
        for member_id in &series.occurrences {
            let Some(t) = by_id.get(member_id) else { continue };
            let rec = desired.entry(*member_id).or_insert_with(|| blank(t));
            rec.is_recurring = true;
            rec.recurring_series_id = Some(series.series_id);
            rec.recurring_cadence = Some(wire_cadence(series.cadence));
            rec.recurring_median_amount = Some(series.median_amount);
        }
    }

    desired
}

/// Runs one detection pass over every account (§3): internal-transfer
/// matching, then recurring-cost detection, publishing only the
/// `transaction_insight` rows that changed — new flags, changed flags, and
/// un-flagging a transaction whose pair/series no longer holds (§3.1/§3.2
/// "un-flagging is as mandatory as flagging").
pub async fn run_detection_pass(
    db: &impl ConnectionTrait,
    publisher: &EventPublisher,
) -> Result<(), DbErr> {
    let settings = match Settings::from_env() {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "detect: failed to load settings, skipping detection pass");
            return Ok(());
        }
    };

    let txns = load_txn_facts(db).await?;
    if txns.is_empty() {
        return Ok(());
    }

    let transfer_cfg = TransferConfig {
        match_days: settings.transfer_match_days,
    };
    let recurring_cfg = RecurringConfig {
        min_occurrences: settings.recurring_min_occurrences,
        amount_tolerance: settings.recurring_amount_tolerance,
        window_months: settings.recurring_window_months,
    };

    let now = Utc::now();
    let desired = build_desired_records(&txns, transfer_cfg, recurring_cfg, now);

    // Every transaction that currently has a flagged row: a desired record
    // absent from this set (because neither detector flagged it this pass)
    // must be un-flagged — published as `is_transfer = false, is_recurring
    // = false` rather than tombstoned, since the row still legitimately
    // exists with its `DEFAULT false` columns (§2.3).
    let existing_ids: HashSet<Uuid> = transaction_insight::Entity::find()
        .filter(
            transaction_insight::Column::IsTransfer
                .eq(true)
                .or(transaction_insight::Column::IsRecurring.eq(true)),
        )
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.transaction_id)
        .collect();

    let mut to_publish: Vec<InsightRecord> = desired.values().cloned().collect();
    for id in &existing_ids {
        if desired.contains_key(id) {
            continue;
        }
        let Some(t) = txns.iter().find(|t| t.id == *id) else {
            continue;
        };
        to_publish.push(InsightRecord {
            schema_version: crate::kafka::insights::CURRENT_SCHEMA_VERSION,
            source: t.source.clone(),
            external_id: t.external_id.clone(),
            is_transfer: false,
            transfer_counterpart: None,
            transfer_match: None,
            is_recurring: false,
            recurring_series_id: None,
            recurring_cadence: None,
            recurring_median_amount: None,
            detected_at: now,
            revision: now,
        });
    }

    for record in to_publish {
        let transaction_id =
            crate::kafka::envelope::transaction_uuid(&record.source, &record.external_id);

        // Compare-before-publish (§2.3): skip entirely when the record
        // would be identical to what is already projected, ignoring the
        // always-fresh `detected_at`/`revision` timestamps.
        if let Ok(Some(existing)) = transaction_insight::Entity::find_by_id(transaction_id)
            .one(db)
            .await
            && unchanged(&existing, &record)
        {
            continue;
        }

        let key = format!("{}:{}", record.source, record.external_id);
        let value = match serde_json::to_vec(&record) {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, transaction_id = %transaction_id, "detect: failed to serialize insight record, not publishing");
                continue;
            }
        };
        if let Err(e) = publisher
            .publish_with_headers(TOPIC_TRANSACTION_INSIGHT, &key, &value, headers())
            .await
        {
            error!(error = %e, transaction_id = %transaction_id, "detect: failed to publish transaction-insight");
        }
        project_insight(db, transaction_id, Some(record)).await?;
    }

    Ok(())
}

fn unchanged(existing: &transaction_insight::Model, record: &InsightRecord) -> bool {
    let counterpart_id = record
        .transfer_counterpart
        .as_deref()
        .and_then(|c| c.split_once(':'))
        .map(|(s, e)| crate::kafka::envelope::transaction_uuid(s, e));
    existing.is_transfer == record.is_transfer
        && existing.transfer_counterpart_id == counterpart_id
        && existing.transfer_match.as_deref()
            == record.transfer_match.map(|m| match m {
                WireMatch::Iban => "iban",
                WireMatch::AmountDate => "amount_date",
            })
        && existing.is_recurring == record.is_recurring
        && existing.recurring_series_id == record.recurring_series_id
        && existing.recurring_cadence.as_deref()
            == record.recurring_cadence.map(|c| match c {
                WireCadence::Monthly => "monthly",
                WireCadence::Quarterly => "quarterly",
                WireCadence::Yearly => "yearly",
            })
        && existing.recurring_median_amount == record.recurring_median_amount
}
