//! Writes normalized records into the read model with the §2.3/§2.8
//! precedence and ownership rules: raw beats reconstructed in either arrival
//! order, a reconstructed record never overwrites a raw one, a transaction's
//! `account_id` never moves once set (first writer owns the link), and a
//! balance/transaction for an account not yet seen gets a stub account
//! instead of a foreign-key failure.

use chrono::{DateTime, Utc};
use entity::entities::{account, account_balance, transaction, user_account};
use sea_orm::sea_query::{Expr, IntoIden, OnConflict, SimpleExpr};
use sea_orm::{ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait};
use uuid::Uuid;

use crate::kafka::envelope::account_uuid;
use crate::projection::records::{AccountRecord, BalanceRecord, TransactionRecord, ORIGIN_SOURCE};

/// `ON CONFLICT ... DO UPDATE ... WHERE <target>.origin <> 'source'` (§2.8):
/// applied whenever the incoming record is a reconstruction, so a re-run
/// backfill can never clobber a row a raw record already claimed. A `source`
/// record carries no guard — it always wins, in either arrival order.
///
/// `origin` is qualified with its own table rather than left bare: Postgres
/// considers a bare column name in this `WHERE` ambiguous between the target
/// row and `excluded` (the conflicting row being inserted), even though only
/// one of them is actually in scope there.
fn legacy_guard<E: IntoIden + Copy + 'static, C: ColumnTrait>(entity: E, origin_column: C) -> SimpleExpr {
    Expr::col((entity, origin_column)).ne(ORIGIN_SOURCE)
}

/// Whether an `account` row with this id already exists — the caller uses
/// this to decide whether `link_default_owner` should run (§4): only the
/// record that first creates the row (real or stub) should (re-)link the
/// configured default owner, so a manual `user-admin unlink` sticks across
/// every later record for that account, including a full replay.
pub async fn account_exists(txn: &impl ConnectionTrait, id: Uuid) -> Result<bool, DbErr> {
    Ok(account::Entity::find_by_id(id).one(txn).await?.is_some())
}

/// Upserts one `account` row (§3, §2.8). `first_seen_at` is excluded from the
/// `DO UPDATE SET` list on purpose — a later observation must not rewrite
/// when this account was first seen.
pub async fn upsert_account(
    txn: &impl ConnectionTrait,
    record: &AccountRecord,
) -> Result<(), DbErr> {
    let model = account::ActiveModel {
        id: Set(record.id),
        source: Set(record.source.clone()),
        external_id: Set(record.external_id.clone()),
        display_id: Set(record.display_id.clone()),
        account_type: Set(record.account_type.clone()),
        iban: Set(record.iban.clone()),
        bic: Set(record.bic.clone()),
        institute: Set(record.institute.clone()),
        label: Set(record.label.clone()),
        currency: Set(record.currency.clone()),
        raw_payload: Set(record.raw_payload.clone()),
        origin: Set(record.origin.clone()),
        first_seen_at: Set(record.first_seen_at.into()),
        updated_at: Set(record.updated_at.into()),
    };

    let mut on_conflict = OnConflict::columns([account::Column::Source, account::Column::ExternalId]);
    on_conflict.update_columns([
        account::Column::DisplayId,
        account::Column::AccountType,
        account::Column::Iban,
        account::Column::Bic,
        account::Column::Institute,
        account::Column::Label,
        account::Column::Currency,
        account::Column::RawPayload,
        account::Column::Origin,
        account::Column::UpdatedAt,
    ]);
    if record.origin != ORIGIN_SOURCE {
        on_conflict.action_cond_where(legacy_guard(account::Entity, account::Column::Origin));
    }

    account::Entity::insert(model)
        .on_conflict(on_conflict.to_owned())
        .exec(txn)
        .await?;

    Ok(())
}

/// Inserts a placeholder `account` row (`origin = 'stub'`, §2.3) so a balance
/// or transaction for an account the projector has not projected yet can
/// still satisfy the foreign key. Never overwrites an existing row of any
/// origin — a stub is the lowest-precedence record there is.
pub async fn ensure_stub_account(
    txn: &impl ConnectionTrait,
    source: &str,
    source_account_id: &str,
    currency: &str,
    seen_at: DateTime<Utc>,
) -> Result<Uuid, DbErr> {
    let id = account_uuid(source, source_account_id);
    let model = account::ActiveModel {
        id: Set(id),
        source: Set(source.to_string()),
        external_id: Set(source_account_id.to_string()),
        display_id: Set(None),
        account_type: Set(None),
        iban: Set(None),
        bic: Set(None),
        institute: Set(None),
        label: Set(None),
        currency: Set(currency.to_string()),
        raw_payload: Set(None),
        origin: Set("stub".to_string()),
        first_seen_at: Set(seen_at.into()),
        updated_at: Set(seen_at.into()),
    };

    account::Entity::insert(model)
        .on_conflict(
            OnConflict::columns([account::Column::Source, account::Column::ExternalId])
                .do_nothing()
                .to_owned(),
        )
        .exec_without_returning(txn)
        .await?;

    Ok(id)
}

/// Upserts one `account_balance` row (§3, §2.8). The conflict target
/// (`account_id`, `balance_date`) is also the row's whole identity, so every
/// other column is safe to overwrite unconditionally for a `source` record —
/// "later observation for the same day wins".
pub async fn upsert_balance(
    txn: &impl ConnectionTrait,
    record: &BalanceRecord,
) -> Result<(), DbErr> {
    let model = account_balance::ActiveModel {
        id: Set(record.id),
        account_id: Set(record.account_id),
        balance_date: Set(record.balance_date),
        amount: Set(record.amount),
        currency: Set(record.currency.clone()),
        raw_payload: Set(record.raw_payload.clone()),
        origin: Set(record.origin.clone()),
        observed_at: Set(record.observed_at.into()),
    };

    let mut on_conflict = OnConflict::columns([
        account_balance::Column::AccountId,
        account_balance::Column::BalanceDate,
    ]);
    on_conflict.update_columns([
        account_balance::Column::Amount,
        account_balance::Column::Currency,
        account_balance::Column::RawPayload,
        account_balance::Column::Origin,
        account_balance::Column::ObservedAt,
    ]);
    if record.origin != ORIGIN_SOURCE {
        on_conflict.action_cond_where(legacy_guard(
            account_balance::Entity,
            account_balance::Column::Origin,
        ));
    }

    account_balance::Entity::insert(model)
        .on_conflict(on_conflict.to_owned())
        .exec(txn)
        .await?;

    Ok(())
}

/// Upserts one `transaction` row (§3, §2.8, §2.3 "first writer owns the
/// account link"). `account_id` is never in the `DO UPDATE SET` list — not
/// even for a `source` record overwriting another `source` record — so the
/// row stays attached to whichever login's `source_account_id` claimed this
/// `reference` first, rather than flapping between two accounts on a joint
/// account seen through two logins.
pub async fn upsert_transaction(
    txn: &impl ConnectionTrait,
    record: &TransactionRecord,
) -> Result<(), DbErr> {
    let model = transaction::ActiveModel {
        id: Set(record.id),
        account_id: Set(record.account_id),
        source: Set(record.source.clone()),
        external_id: Set(record.external_id.clone()),
        booking_date: Set(record.booking_date),
        valuta_date: Set(record.valuta_date),
        booking_status: Set(record.booking_status.clone()),
        amount: Set(record.amount),
        currency: Set(record.currency.clone()),
        counterparty_name: Set(record.counterparty_name.clone()),
        counterparty_iban: Set(record.counterparty_iban.clone()),
        description: Set(record.description.clone()),
        transaction_type: Set(record.transaction_type.clone()),
        raw_payload: Set(record.raw_payload.clone()),
        origin: Set(record.origin.clone()),
        imported_at: Set(record.imported_at.into()),
        updated_at: Set(record.updated_at.into()),
        counterparty_key: Set(record.counterparty_key.clone()),
    };

    let mut on_conflict =
        OnConflict::columns([transaction::Column::Source, transaction::Column::ExternalId]);
    on_conflict.update_columns([
        transaction::Column::BookingDate,
        transaction::Column::ValutaDate,
        transaction::Column::BookingStatus,
        transaction::Column::Amount,
        transaction::Column::Currency,
        transaction::Column::CounterpartyName,
        transaction::Column::CounterpartyIban,
        transaction::Column::Description,
        transaction::Column::TransactionType,
        transaction::Column::RawPayload,
        transaction::Column::Origin,
        transaction::Column::ImportedAt,
        transaction::Column::UpdatedAt,
        transaction::Column::CounterpartyKey,
    ]);
    if record.origin != ORIGIN_SOURCE {
        on_conflict.action_cond_where(legacy_guard(
            transaction::Entity,
            transaction::Column::Origin,
        ));
    }

    transaction::Entity::insert(model)
        .on_conflict(on_conflict.to_owned())
        .exec(txn)
        .await?;

    Ok(())
}

/// Links `account_id` to `user_id` in `user_account` (§4's
/// `APP_projector_default_owner`), idempotently — a replay must not fail on
/// a link the previous run already made.
pub async fn link_default_owner(
    txn: &impl ConnectionTrait,
    user_id: Uuid,
    account_id: Uuid,
    created_at: DateTime<Utc>,
) -> Result<(), DbErr> {
    let model = user_account::ActiveModel {
        user_id: Set(user_id),
        account_id: Set(account_id),
        created_at: Set(created_at.into()),
    };

    user_account::Entity::insert(model)
        .on_conflict(
            OnConflict::columns([user_account::Column::UserId, user_account::Column::AccountId])
                .do_nothing()
                .to_owned(),
        )
        .exec_without_returning(txn)
        .await?;

    Ok(())
}

/// Finds the stored `next_offset` under `group_id` for the account/account-balance/transaction
/// topics the projector tracks, keyed by `(topic, partition)` — exposed here
/// too (alongside `offsets::load_offsets`) so `upsert.rs`'s integration tests
/// can assert on committed offsets without importing the whole `offsets`
/// module's write path.
/// Available to unit tests directly and to the `integration`-gated
/// integration tests (a separate test binary, hence `pub` rather than
/// `pub(crate)`) — never compiled into the real binary otherwise.
#[cfg(any(test, feature = "integration"))]
pub async fn offset_for(
    db: &impl ConnectionTrait,
    group_id: &str,
    topic: &str,
    partition: i32,
) -> Result<Option<i64>, DbErr> {
    use entity::entities::projection_offset;
    use sea_orm::QueryFilter;

    let row = projection_offset::Entity::find()
        .filter(projection_offset::Column::GroupId.eq(group_id))
        .filter(projection_offset::Column::Topic.eq(topic))
        .filter(projection_offset::Column::Partition.eq(partition))
        .one(db)
        .await?;
    Ok(row.map(|r| r.next_offset))
}

