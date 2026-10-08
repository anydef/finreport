//! WP-B (iteration 3 §4): `Transaction.tags/transfer/recurring` field
//! resolvers (auto/user merge, §2.2), `Query.tags`/`recurringSeries`, and
//! the `setTransactionTags`/`setTransactionRecurring` mutations —
//! read-modify-write publish-then-upsert on `finreport.user-label` (§2.1),
//! same shape as `labels.rs`'s iteration-2 mutations.

use async_graphql::ErrorExtensions;
use chrono::{NaiveDate, Utc};
use entity::entities::{transaction, transaction_insight, transaction_tag, transaction_user_label};
use rust_decimal::Decimal as RustDecimal;
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Statement};
use std::sync::Arc;
use uuid::Uuid;

use crate::graphql::events::{kafka_unavailable_error, publish_event};
use crate::graphql::labels::load_scoped_transaction;
use crate::graphql::scalars::{Date as GqlDate, Decimal as GqlDecimal, Uuid as GqlUuid};
use crate::graphql::types::{
    Direction, FlagSource, RecurringCadence, RecurringInfo, RecurringOverview, RecurringSeries,
    TagCount, Transaction, TransactionFilter, TransferInfo, TransferMatch,
};
use crate::kafka::labeling::{
    SplitPart, UserLabelRecord, TOPIC_USER_LABEL, USER_LABEL_SCHEMA_VERSION_V2,
};
use crate::kafka::producer::EventPublisher;



// ---------------------------------------------------------------------------
// §4 tag normalization
// ---------------------------------------------------------------------------

/// Folds halfwidth/fullwidth Unicode variants (the common NFKC case for
/// user-entered tags, e.g. fullwidth Latin letters/digits) to their plain
/// ASCII form. Deviation from a full NFKC implementation (§4): adding
/// `unicode-normalization` as a direct dependency means touching
/// `webapp/Cargo.toml`, outside this work package's owned file set
/// (`webapp/src/graphql/**` only) — this covers the fullwidth-ASCII block
/// (U+FF01..U+FF5E, the overwhelmingly common real-world case) without it.
fn fold_compatibility(ch: char) -> char {
    match ch {
        '\u{FF01}'..='\u{FF5E}' => {
            char::from_u32(ch as u32 - 0xFEE0).unwrap_or(ch)
        }
        '\u{3000}' => ' ', // ideographic space
        other => other,
    }
}

/// Normalizes one raw tag per §4: NFKC (see [`fold_compatibility`]),
/// lowercase, trim, collapse internal whitespace and `_` to `-`, strip
/// anything outside `[a-z0-9-]`, collapse repeated `-`, trim leading/
/// trailing `-`. Empty after normalization ⇒ `None`; length must be
/// `1..=32`.
pub fn normalize_tag(raw: &str) -> Option<String> {
    let folded: String = raw.chars().map(fold_compatibility).collect();
    let lowered = folded.to_lowercase();
    let mut collapsed = String::with_capacity(lowered.len());
    let mut last_was_sep = false;
    for ch in lowered.trim().chars() {
        let mapped = if ch.is_whitespace() || ch == '_' || ch == '-' {
            Some('-')
        } else if ch.is_ascii_alphanumeric() {
            Some(ch)
        } else {
            None
        };
        match mapped {
            Some('-') => {
                if !last_was_sep && !collapsed.is_empty() {
                    collapsed.push('-');
                }
                last_was_sep = true;
            }
            Some(c) => {
                collapsed.push(c);
                last_was_sep = false;
            }
            None => {}
        }
    }
    let trimmed = collapsed.trim_matches('-');
    if trimmed.is_empty() || trimmed.chars().count() > 32 {
        return None;
    }
    Some(trimmed.to_string())
}

/// Normalizes, dedupes and sorts a raw tag list (§4). Returns
/// `Err(TOO_MANY_TAGS)` when the result exceeds `max_tags`.
pub fn normalize_tags(raw: &[String], max_tags: u32) -> async_graphql::Result<Vec<String>> {
    let mut tags: Vec<String> = raw.iter().filter_map(|t| normalize_tag(t)).collect();
    tags.sort();
    tags.dedup();
    if tags.len() as u32 > max_tags {
        return Err(async_graphql::Error::new(format!(
            "at most {max_tags} tags are allowed per transaction, got {}",
            tags.len()
        ))
        .extend_with(|_, e| e.set("code", "TOO_MANY_TAGS")));
    }
    Ok(tags)
}

// ---------------------------------------------------------------------------
// §2.2 auto/user merge helpers
// ---------------------------------------------------------------------------

fn gql_cadence(cadence: &str) -> Option<RecurringCadence> {
    match cadence {
        "monthly" => Some(RecurringCadence::Monthly),
        "quarterly" => Some(RecurringCadence::Quarterly),
        "yearly" => Some(RecurringCadence::Yearly),
        _ => None,
    }
}

fn gql_transfer_match(value: &str) -> TransferMatch {
    match value {
        "iban" => TransferMatch::Iban,
        _ => TransferMatch::AmountDate,
    }
}

fn cadence_months(cadence: RecurringCadence) -> i32 {
    match cadence {
        RecurringCadence::Monthly => 1,
        RecurringCadence::Quarterly => 3,
        RecurringCadence::Yearly => 12,
    }
}

fn add_months(date: NaiveDate, months: i32) -> NaiveDate {
    use chrono::Datelike;
    let total_months = date.year() * 12 + date.month0() as i32 + months;
    let year = total_months.div_euclid(12);
    let month0 = total_months.rem_euclid(12);
    let day = date.day();
    // Clamp to the target month's last day (e.g. Jan 31 + 1 month -> Feb 28/29).
    for d in (1..=day).rev() {
        if let Some(found) = NaiveDate::from_ymd_opt(year, month0 as u32 + 1, d) {
            return found;
        }
    }
    NaiveDate::from_ymd_opt(year, month0 as u32 + 1, 1).unwrap()
}

/// `Transaction.tags` (§4): sorted, `[]` when untagged.
pub async fn tags_for(
    db: &DatabaseConnection,
    transaction_id: Uuid,
) -> async_graphql::Result<Vec<String>> {
    let mut rows = transaction_tag::Entity::find()
        .filter(transaction_tag::Column::TransactionId.eq(transaction_id))
        .all(db)
        .await?;
    rows.sort_by(|a, b| a.tag.cmp(&b.tag));
    Ok(rows.into_iter().map(|r| r.tag).collect())
}

/// `Transaction.transfer` (§4): `null` = not an internal transfer. Transfers
/// have no override layer (§2.2), so this reads `transaction_insight`
/// directly.
pub async fn transfer_for(
    db: &DatabaseConnection,
    transaction_id: Uuid,
) -> async_graphql::Result<Option<TransferInfo>> {
    let Some(insight) = transaction_insight::Entity::find_by_id(transaction_id)
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    if !insight.is_transfer {
        return Ok(None);
    }
    let counterpart_account_id = match insight.transfer_counterpart_id {
        Some(counterpart_id) => transaction::Entity::find_by_id(counterpart_id)
            .one(db)
            .await?
            .map(|t| GqlUuid(t.account_id)),
        None => None,
    };
    Ok(Some(TransferInfo {
        counterpart_transaction_id: insight.transfer_counterpart_id.map(GqlUuid),
        counterpart_account_id,
        match_: insight
            .transfer_match
            .as_deref()
            .map(gql_transfer_match)
            .unwrap_or(TransferMatch::AmountDate),
    }))
}

/// `Transaction.recurring` (§4, §2.2): always present. A user override
/// (`transaction_user_label.recurring`) wins over the detector's
/// `transaction_insight.is_recurring`; `seriesId`/`cadence`/`medianAmount`
/// are `null` whenever the effective flag came from an override — an
/// override severs the link to any detected series (§4 "seriesId null when
/// overridden in/out of a series").
pub async fn recurring_for(
    db: &DatabaseConnection,
    transaction_id: Uuid,
) -> async_graphql::Result<RecurringInfo> {
    let user_recurring = transaction_user_label::Entity::find_by_id(transaction_id)
        .one(db)
        .await?
        .and_then(|r| r.recurring);
    let insight = transaction_insight::Entity::find_by_id(transaction_id)
        .one(db)
        .await?;

    if let Some(overridden) = user_recurring {
        return Ok(RecurringInfo {
            is_recurring: overridden,
            source: FlagSource::User,
            series_id: None,
            cadence: None,
            median_amount: None,
        });
    }

    match insight {
        Some(insight) if insight.is_recurring => Ok(RecurringInfo {
            is_recurring: true,
            source: FlagSource::Auto,
            series_id: insight.recurring_series_id.map(GqlUuid),
            cadence: insight.recurring_cadence.as_deref().and_then(gql_cadence),
            median_amount: insight.recurring_median_amount.map(GqlDecimal),
        }),
        _ => Ok(RecurringInfo {
            is_recurring: false,
            source: FlagSource::Auto,
            series_id: None,
            cadence: None,
            median_amount: None,
        }),
    }
}

// ---------------------------------------------------------------------------
// Query.tags
// ---------------------------------------------------------------------------

/// `tags` (§4): all tags used by the caller's own transactions, descending
/// count.
pub async fn fetch_tag_counts(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
) -> async_graphql::Result<Vec<TagCount>> {
    if scoped_ids.is_empty() {
        return Ok(Vec::new());
    }
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT tt.tag AS tag, COUNT(*) AS transaction_count \
         FROM transaction_tag tt \
         JOIN transaction t ON t.id = tt.transaction_id \
         WHERE t.account_id = ANY($1) \
         GROUP BY tt.tag \
         ORDER BY transaction_count DESC, tt.tag ASC",
        vec![scoped_ids.to_vec().into()],
    );
    let rows = db.query_all(stmt).await?;
    rows.iter()
        .map(|row| {
            Ok(TagCount {
                tag: row.try_get("", "tag")?,
                transaction_count: {
                    let count: i64 = row.try_get("", "transaction_count")?;
                    count as i32
                },
            })
        })
        .collect::<Result<Vec<_>, sea_orm::DbErr>>()
        .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Query.recurringSeries
// ---------------------------------------------------------------------------

struct SeriesAccumulator {
    counterparty_key: String,
    counterparty_name: Option<String>,
    cadence: RecurringCadence,
    median_amount: RustDecimal,
    occurrence_count: i32,
    first_date: NaiveDate,
    last_date: NaiveDate,
}

/// `recurringSeries` (§4): honours the date/account/category parts of
/// `filter` when deciding *which series are returned* (a series needs at
/// least one member matching the filter), but reports each series' full
/// extent — computed over *all* of its members, not just the filtered ones
/// (§4: "a one-month window must not make a yearly series look like a
/// single payment").
pub async fn fetch_recurring_overview(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
) -> async_graphql::Result<RecurringOverview> {
    if scoped_ids.is_empty() {
        return Ok(RecurringOverview {
            series: Vec::new(),
            total_monthly_equivalent: GqlDecimal(RustDecimal::ZERO),
            currency: "EUR".to_string(),
        });
    }

    let currency = entity::entities::account::Entity::find()
        .filter(entity::entities::account::Column::Id.is_in(scoped_ids.to_vec()))
        .all(db)
        .await?
        .into_iter()
        .map(|a| a.currency)
        .next()
        .unwrap_or_else(|| "EUR".to_string());

    // Which series have at least one member matching the filter.
    let condition =
        crate::graphql::transactions::build_condition_with_category_filters(db, scoped_ids, filter)
            .await?;
    let matching_ids: Vec<Uuid> = transaction::Entity::find()
        .filter(condition)
        .all(db)
        .await?
        .into_iter()
        .map(|t| t.id)
        .collect();
    if matching_ids.is_empty() {
        return Ok(RecurringOverview {
            series: Vec::new(),
            total_monthly_equivalent: GqlDecimal(RustDecimal::ZERO),
            currency,
        });
    }
    let matching_series_ids: Vec<Uuid> = transaction_insight::Entity::find()
        .filter(transaction_insight::Column::TransactionId.is_in(matching_ids))
        .filter(transaction_insight::Column::RecurringSeriesId.is_not_null())
        .all(db)
        .await?
        .into_iter()
        .filter_map(|i| i.recurring_series_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    if matching_series_ids.is_empty() {
        return Ok(RecurringOverview {
            series: Vec::new(),
            total_monthly_equivalent: GqlDecimal(RustDecimal::ZERO),
            currency,
        });
    }

    // Full, unfiltered membership (scoped to the caller's own accounts) for
    // every matched series.
    let members = transaction_insight::Entity::find()
        .filter(transaction_insight::Column::RecurringSeriesId.is_in(matching_series_ids))
        .all(db)
        .await?;
    let member_ids: Vec<Uuid> = members.iter().map(|m| m.transaction_id).collect();
    let txns: std::collections::HashMap<Uuid, transaction::Model> = transaction::Entity::find()
        .filter(transaction::Column::Id.is_in(member_ids))
        .filter(transaction::Column::AccountId.is_in(scoped_ids.to_vec()))
        .all(db)
        .await?
        .into_iter()
        .map(|t| (t.id, t))
        .collect();

    let mut accumulators: std::collections::HashMap<Uuid, SeriesAccumulator> =
        std::collections::HashMap::new();
    for member in &members {
        let Some(series_id) = member.recurring_series_id else {
            continue;
        };
        let Some(txn) = txns.get(&member.transaction_id) else {
            // Not owned by the caller's scoped accounts: skip.
            continue;
        };
        let cadence = member
            .recurring_cadence
            .as_deref()
            .and_then(gql_cadence)
            .unwrap_or(RecurringCadence::Monthly);
        let median_amount = member.recurring_median_amount.unwrap_or(RustDecimal::ZERO);
        accumulators
            .entry(series_id)
            .and_modify(|acc| {
                acc.occurrence_count += 1;
                acc.first_date = acc.first_date.min(txn.booking_date);
                acc.last_date = acc.last_date.max(txn.booking_date);
                if txn.booking_date >= acc.last_date {
                    acc.counterparty_name = txn.counterparty_name.clone();
                }
            })
            .or_insert_with(|| SeriesAccumulator {
                counterparty_key: txn.counterparty_key.clone().unwrap_or_default(),
                counterparty_name: txn.counterparty_name.clone(),
                cadence,
                median_amount,
                occurrence_count: 1,
                first_date: txn.booking_date,
                last_date: txn.booking_date,
            });
    }

    let today = Utc::now().date_naive();
    let mut series: Vec<RecurringSeries> = accumulators
        .into_iter()
        .map(|(series_id, acc)| {
            let next_expected_date = add_months(acc.last_date, cadence_months(acc.cadence));
            let monthly_equivalent = monthly_equivalent(acc.median_amount, acc.cadence);
            RecurringSeries {
                id: GqlUuid(series_id),
                counterparty_key: acc.counterparty_key,
                counterparty_name: acc.counterparty_name,
                direction: if acc.median_amount.is_sign_positive() {
                    Direction::Income
                } else {
                    Direction::Spending
                },
                cadence: acc.cadence,
                median_amount: GqlDecimal(acc.median_amount),
                monthly_equivalent: GqlDecimal(monthly_equivalent),
                occurrence_count: acc.occurrence_count,
                first_date: GqlDate(acc.first_date),
                last_date: GqlDate(acc.last_date),
                next_expected_date: GqlDate(next_expected_date),
                stale: next_expected_date < today,
            }
        })
        .collect();
    series.sort_by(|a, b| {
        b.monthly_equivalent
            .0
            .abs()
            .cmp(&a.monthly_equivalent.0.abs())
            .then_with(|| a.counterparty_key.cmp(&b.counterparty_key))
    });

    let total_monthly_equivalent: RustDecimal = series
        .iter()
        .filter(|s| s.median_amount.0.is_sign_negative())
        .map(|s| s.monthly_equivalent.0.abs())
        .sum();

    Ok(RecurringOverview {
        series,
        total_monthly_equivalent: GqlDecimal(total_monthly_equivalent),
        currency,
    })
}

/// Monthly-equivalent cost (§3.2): `medianAmount × {monthly 1, quarterly
/// 1/3, yearly 1/12}`, exact decimal, half-up to 4 dp.
fn monthly_equivalent(median_amount: RustDecimal, cadence: RecurringCadence) -> RustDecimal {
    let divisor = match cadence {
        RecurringCadence::Monthly => RustDecimal::ONE,
        RecurringCadence::Quarterly => RustDecimal::from(3),
        RecurringCadence::Yearly => RustDecimal::from(12),
    };
    let mut result = median_amount / divisor;
    result.rescale(4);
    result
}

// ---------------------------------------------------------------------------
// Mutations: read-modify-write publish-then-upsert (§2.1)
// ---------------------------------------------------------------------------

/// Current state of a transaction's whole-state `transaction_user_label`
/// record (§2.1), loaded so a mutation that only touches one field can
/// republish the rest unchanged.
pub(crate) struct CurrentUserLabelState {
    category_slug: Option<String>,
    parts: Vec<SplitPart>,
    tags: Vec<String>,
    recurring: Option<bool>,
    note: Option<String>,
}

/// Loads the current whole-state record for `transaction_id` (§2.1): the
/// category slug (resolved from `transaction_user_label.category_id`), any
/// splits, the tag set, the recurring override and the note — everything
/// `UserLabelRecord` carries, so a caller can change one field and
/// republish the rest unchanged.
pub(crate) async fn load_current_state(
    db: &DatabaseConnection,
    transaction_id: Uuid,
) -> async_graphql::Result<CurrentUserLabelState> {
    let user_label = transaction_user_label::Entity::find_by_id(transaction_id)
        .one(db)
        .await?;
    let category_slug = match user_label.as_ref().and_then(|r| r.category_id) {
        Some(category_id) => entity::entities::category::Entity::find_by_id(category_id)
            .one(db)
            .await?
            .map(|c| c.slug),
        None => None,
    };
    let split_rows = entity::entities::transaction_split::Entity::find()
        .filter(entity::entities::transaction_split::Column::TransactionId.eq(transaction_id))
        .all(db)
        .await?;
    let mut parts = Vec::with_capacity(split_rows.len());
    for row in &split_rows {
        let category = entity::entities::category::Entity::find_by_id(row.category_id)
            .one(db)
            .await?;
        if let Some(category) = category {
            parts.push(SplitPart {
                index: row.part_index,
                amount: row.amount,
                category_slug: category.slug,
            });
        }
    }
    let tags = tags_for(db, transaction_id).await?;
    let recurring = user_label.as_ref().and_then(|r| r.recurring);
    let note = user_label.and_then(|r| r.note);
    Ok(CurrentUserLabelState {
        category_slug,
        parts,
        tags,
        recurring,
        note,
    })
}

async fn publish_user_label(
    publisher: &Arc<EventPublisher>,
    record: &UserLabelRecord,
) -> async_graphql::Result<()> {
    let key = format!("{}:{}", record.source, record.external_id);
    let value = serde_json::to_vec(record)
        .map_err(|e| async_graphql::Error::new(format!("failed to serialize user-label: {e}")))?;
    publish_event(publisher, TOPIC_USER_LABEL, &key, &value).await
}

/// Upserts `transaction_user_label` (category, note, recurring), guarded by
/// `revision` (§2.1), mirroring `labels.rs`'s `upsert_user_label` but also
/// carrying `recurring` — the column that mutation's original version
/// dropped (WP0 addendum §9.7; fixed here with the rest of the
/// read-modify-write).
async fn upsert_user_label_row(
    db: &DatabaseConnection,
    transaction_id: Uuid,
    category_id: Option<Uuid>,
    note: Option<&str>,
    recurring: Option<bool>,
    revision: chrono::DateTime<Utc>,
) -> async_graphql::Result<bool> {
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        r#"INSERT INTO transaction_user_label (transaction_id, category_id, note, recurring, revision)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (transaction_id) DO UPDATE SET
             category_id = excluded.category_id, note = excluded.note,
             recurring = excluded.recurring, revision = excluded.revision
           WHERE transaction_user_label.revision <= excluded.revision"#,
        vec![
            transaction_id.into(),
            category_id.into(),
            note.into(),
            recurring.into(),
            revision.into(),
        ],
    );
    let result = db.execute(stmt).await?;
    Ok(result.rows_affected() > 0)
}

/// Replaces the transaction's whole tag set (§2.3): delete what the new set
/// omits, insert the rest `ON CONFLICT DO NOTHING`.
async fn replace_tags(
    db: &DatabaseConnection,
    transaction_id: Uuid,
    tags: &[String],
    revision: chrono::DateTime<Utc>,
) -> async_graphql::Result<()> {
    if tags.is_empty() {
        transaction_tag::Entity::delete_many()
            .filter(transaction_tag::Column::TransactionId.eq(transaction_id))
            .exec(db)
            .await?;
        return Ok(());
    }
    transaction_tag::Entity::delete_many()
        .filter(transaction_tag::Column::TransactionId.eq(transaction_id))
        .filter(transaction_tag::Column::Tag.is_not_in(tags.to_vec()))
        .exec(db)
        .await?;
    for tag in tags {
        transaction_tag::Entity::insert(transaction_tag::ActiveModel {
            transaction_id: sea_orm::Set(transaction_id),
            tag: sea_orm::Set(tag.clone()),
            revision: sea_orm::Set(revision.into()),
        })
        .on_conflict(
            sea_orm::sea_query::OnConflict::columns([
                transaction_tag::Column::TransactionId,
                transaction_tag::Column::Tag,
            ])
            .do_nothing()
            .to_owned(),
        )
        .exec(db)
        .await?;
    }
    Ok(())
}

fn to_graphql_transaction(row: &transaction::Model) -> Transaction {
    Transaction {
        id: GqlUuid(row.id),
        account_id: GqlUuid(row.account_id),
        source: row.source.clone(),
        external_id: row.external_id.clone(),
        booking_date: GqlDate(row.booking_date),
        valuta_date: row.valuta_date.map(GqlDate),
        booking_status: row.booking_status.clone(),
        amount: GqlDecimal(row.amount),
        currency: row.currency.clone(),
        counterparty_name: row.counterparty_name.clone(),
        counterparty_iban: row.counterparty_iban.clone(),
        description: row.description.clone(),
        transaction_type: row.transaction_type.clone(),
        counterparty_key: row.counterparty_key.clone(),
    }
}

/// `setTransactionTags` (§4): replaces the whole tag set, `[]` clears.
pub async fn set_transaction_tags(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    scoped_ids: &[Uuid],
    transaction_id: Uuid,
    tags: Vec<String>,
    max_tags: u32,
) -> async_graphql::Result<Transaction> {
    let txn = load_scoped_transaction(db, scoped_ids, transaction_id).await?;
    let normalized = normalize_tags(&tags, max_tags)?;
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;
    apply_tags(db, publisher, &txn, &normalized).await?;
    Ok(to_graphql_transaction(&txn))
}

/// The publish-then-upsert core of `setTransactionTags`, shared with the
/// bulk mutation (`bulk.rs`). `normalized` must already have passed
/// [`normalize_tags`]. Preserves the category, splits, recurring override
/// and note by republishing them from the current state (§2.1).
///
/// Returns whether the write applied (`false` ⇒ a newer `revision` won).
pub(crate) async fn apply_tags(
    db: &DatabaseConnection,
    publisher: &Arc<EventPublisher>,
    txn: &transaction::Model,
    normalized: &[String],
) -> async_graphql::Result<bool> {
    let transaction_id = txn.id;
    let current = load_current_state(db, transaction_id).await?;
    let revision = Utc::now();
    let record = UserLabelRecord {
        schema_version: USER_LABEL_SCHEMA_VERSION_V2,
        source: txn.source.clone(),
        external_id: txn.external_id.clone(),
        category_slug: current.category_slug.clone(),
        parts: current.parts.clone(),
        tags: normalized.to_vec(),
        recurring: current.recurring,
        revision,
        note: current.note.clone(),
    };
    publish_user_label(publisher, &record).await?;

    let category_id = match &current.category_slug {
        Some(slug) => crate::graphql::categories::find_by_slug(db, slug).await?.map(|c| c.id),
        None => None,
    };
    if upsert_user_label_row(
        db,
        transaction_id,
        category_id,
        current.note.as_deref(),
        current.recurring,
        revision,
    )
    .await?
    {
        replace_tags(db, transaction_id, normalized, revision).await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// `setTransactionRecurring` (§4): `null` clears the override and lets
/// auto-detection decide again.
pub async fn set_transaction_recurring(
    db: &DatabaseConnection,
    publisher: Option<&Arc<EventPublisher>>,
    scoped_ids: &[Uuid],
    transaction_id: Uuid,
    recurring: Option<bool>,
) -> async_graphql::Result<Transaction> {
    let txn = load_scoped_transaction(db, scoped_ids, transaction_id).await?;
    let publisher = publisher.ok_or_else(kafka_unavailable_error)?;

    let current = load_current_state(db, transaction_id).await?;
    let revision = Utc::now();
    let record = UserLabelRecord {
        schema_version: USER_LABEL_SCHEMA_VERSION_V2,
        source: txn.source.clone(),
        external_id: txn.external_id.clone(),
        category_slug: current.category_slug.clone(),
        parts: current.parts.clone(),
        tags: current.tags.clone(),
        recurring,
        revision,
        note: current.note.clone(),
    };
    publish_user_label(publisher, &record).await?;

    let category_id = match &current.category_slug {
        Some(slug) => crate::graphql::categories::find_by_slug(db, slug).await?.map(|c| c.id),
        None => None,
    };
    upsert_user_label_row(
        db,
        transaction_id,
        category_id,
        current.note.as_deref(),
        recurring,
        revision,
    )
    .await?;
    Ok(to_graphql_transaction(&txn))
}

/// Shared by `labels.rs`'s iteration-2 mutations to preserve `tags`/
/// `recurring` across a category/split change (§2.1 read-modify-write;
/// WP0 addendum §9.7 flagged the previous `Vec::new()`/`None` as a bug for
/// WP-B to fix).
pub(crate) async fn preserved_tags_and_recurring(
    db: &DatabaseConnection,
    transaction_id: Uuid,
) -> async_graphql::Result<(Vec<String>, Option<bool>)> {
    let state = load_current_state(db, transaction_id).await?;
    Ok((state.tags, state.recurring))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_tag_lowercases_trims_and_collapses_separators() {
        assert_eq!(normalize_tag("  Italy 2026  "), Some("italy-2026".to_string()));
        assert_eq!(normalize_tag("Hobby__Stuff"), Some("hobby-stuff".to_string()));
        assert_eq!(normalize_tag("--leading-trailing--"), Some("leading-trailing".to_string()));
        assert_eq!(normalize_tag("a!!!b"), Some("ab".to_string()));
        assert_eq!(normalize_tag("   "), None);
        assert_eq!(normalize_tag("---"), None);
    }

    #[test]
    fn normalize_tag_enforces_length_bounds() {
        assert_eq!(normalize_tag(""), None);
        let exactly_32 = "a".repeat(32);
        assert_eq!(normalize_tag(&exactly_32), Some(exactly_32.clone()));
        let too_long = "a".repeat(33);
        assert_eq!(normalize_tag(&too_long), None);
    }

    #[test]
    fn normalize_tag_applies_nfkc_before_folding() {
        // U+FF21 FULLWIDTH LATIN CAPITAL LETTER A -> NFKC -> 'A' -> 'a'.
        assert_eq!(normalize_tag("\u{FF21}bc"), Some("abc".to_string()));
    }

    #[test]
    fn normalize_tags_dedupes_and_sorts() {
        let raw = vec!["Hobby".to_string(), "hobby".to_string(), "Italy 2026".to_string()];
        let tags = normalize_tags(&raw, 10).unwrap();
        assert_eq!(tags, vec!["hobby".to_string(), "italy-2026".to_string()]);
    }

    #[test]
    fn normalize_tags_rejects_more_than_the_configured_limit() {
        let raw: Vec<String> = (0..5).map(|i| format!("tag-{i}")).collect();
        let err = normalize_tags(&raw, 3).unwrap_err();
        assert_eq!(
            err.extensions.unwrap().get("code"),
            Some(&async_graphql::Value::String("TOO_MANY_TAGS".to_string()))
        );
    }

    #[test]
    fn monthly_equivalent_matches_the_documented_divisors() {
        let amount = RustDecimal::from_str_exact("-39.9000").unwrap();
        assert_eq!(
            monthly_equivalent(amount, RecurringCadence::Monthly),
            RustDecimal::from_str_exact("-39.9000").unwrap()
        );
        let quarterly = RustDecimal::from_str_exact("-187.4300").unwrap();
        assert_eq!(
            monthly_equivalent(quarterly, RecurringCadence::Quarterly),
            (quarterly / RustDecimal::from(3)).round_dp(4)
        );
        let yearly = RustDecimal::from_str_exact("-1200.0000").unwrap();
        assert_eq!(
            monthly_equivalent(yearly, RecurringCadence::Yearly),
            RustDecimal::from_str_exact("-100.0000").unwrap()
        );
    }

    #[test]
    fn add_months_clamps_to_the_shorter_target_month() {
        let jan_31 = NaiveDate::from_ymd_opt(2024, 1, 31).unwrap();
        assert_eq!(add_months(jan_31, 1), NaiveDate::from_ymd_opt(2024, 2, 29).unwrap());
        let dec_15 = NaiveDate::from_ymd_opt(2024, 12, 15).unwrap();
        assert_eq!(add_months(dec_15, 1), NaiveDate::from_ymd_opt(2025, 1, 15).unwrap());
    }

    #[test]
    fn effective_flag_truth_table() {
        // user=Some(true) always wins regardless of insight.
        assert_eq!(effective_recurring(Some(true), false), (true, FlagSource::User));
        assert_eq!(effective_recurring(Some(false), true), (false, FlagSource::User));
        // user=None defers to the detector.
        assert_eq!(effective_recurring(None, true), (true, FlagSource::Auto));
        assert_eq!(effective_recurring(None, false), (false, FlagSource::Auto));
    }

    /// Pure helper mirroring `recurring_for`'s merge rule (§2.2), extracted
    /// here only for the truth-table unit test above — `recurring_for`
    /// itself needs a DB connection to also resolve `seriesId`/`cadence`.
    fn effective_recurring(user: Option<bool>, auto: bool) -> (bool, FlagSource) {
        match user {
            Some(value) => (value, FlagSource::User),
            None => (auto, FlagSource::Auto),
        }
    }
}
