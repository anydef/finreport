//! Display aliases: a user's nicknames for merchants and for their own
//! accounts. Display only - nothing the labeler, rules, fingerprints or the
//! recurring detector read is ever rewritten; `Transaction.counterpartyName`
//! stays the bank's string, and the alias is resolved on top of it by
//! `Transaction.counterpartyDisplayName` / `Account.displayName`.
//!
//! Two key kinds share one topic and one table:
//!
//! - `COUNTERPARTY`, keyed by the normalised `counterparty_key`, so one alias
//!   covers every spelling variant of a merchant. An own account the user has
//!   *not* connected only ever appears as a counterparty string on a
//!   connected account's transactions, so it is a merchant for this purpose.
//! - `ACCOUNT`, keyed by the account id, for connected accounts. (The
//!   login-level `Account.label` is shared by every account a login imports,
//!   which is exactly why two accounts look identical.)
//!
//! Aliases are **per user**, unlike rules, categories and learning
//! exemptions: those steer a shared labeler and are documented as global, but
//! a nickname is a personal presentation choice ("Mum" is not another
//! household member's "Mum"), and account aliases name rows only their owner
//! is linked to. The owner is part of the key on topic and table, and every
//! read here filters on the caller.
//!
//! Per-request cost: [`AliasCache`] (registered in `request_with_auth`) loads
//! the caller's whole alias set with one query the first time any display
//! field resolves; every later row of the page reads the same in-memory
//! [`AliasBook`].

use async_graphql::{ComplexObject, Context, Enum, ErrorExtensions, SimpleObject};
use chrono::Utc;
use entity::entities::{account, display_alias, transaction};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::OnceCell;
use uuid::Uuid;

use crate::graphql::current_user::current_user;
use crate::graphql::events::{kafka_unavailable_error, publish_event, publish_tombstone};
use crate::graphql::scalars::DateTime as GqlDateTime;
use crate::graphql::types::{Account, Transaction};
use crate::kafka::labeling::{
    display_alias_topic_key, AliasKind, DisplayAliasRecord, CURRENT_SCHEMA_VERSION,
    TOPIC_DISPLAY_ALIAS,
};
use crate::kafka::producer::EventPublisher;
use crate::projection::display_alias as proj;

/// Longest alias accepted; a nickname, not a description.
pub const MAX_ALIAS_LEN: usize = 80;

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
#[graphql(name = "DisplayAliasKind")]
pub enum GqlAliasKind {
    /// A merchant, or an own account that is not connected: keyed by the
    /// normalised counterparty key.
    Counterparty,
    /// One of the caller's connected accounts: keyed by the account id.
    Account,
}

impl From<GqlAliasKind> for AliasKind {
    fn from(kind: GqlAliasKind) -> Self {
        match kind {
            GqlAliasKind::Counterparty => AliasKind::Counterparty,
            GqlAliasKind::Account => AliasKind::Account,
        }
    }
}

impl From<AliasKind> for GqlAliasKind {
    fn from(kind: AliasKind) -> Self {
        match kind {
            AliasKind::Counterparty => GqlAliasKind::Counterparty,
            AliasKind::Account => GqlAliasKind::Account,
        }
    }
}

/// One nickname, with the original name it replaces.
#[derive(SimpleObject, Clone, Debug)]
pub struct DisplayAlias {
    pub kind: GqlAliasKind,
    /// The normalised counterparty key, or the account id.
    pub key: String,
    pub alias: String,
    /// The name shown when there is no alias: the most frequent spelling on
    /// the caller's transactions (counterparty) or the account's label /
    /// IBAN. Falls back to the key.
    pub raw_name: String,
    /// Caller's transactions carrying this counterparty key; `null` for an
    /// account alias.
    pub transaction_count: Option<i32>,
    pub updated_at: GqlDateTime,
}

fn validation_error(message: impl Into<String>) -> async_graphql::Error {
    async_graphql::Error::new(message.into()).extend_with(|_, e| e.set("code", "VALIDATION"))
}

// ---------------------------------------------------------------------------
// The pure half
// ---------------------------------------------------------------------------

/// A caller's aliases, indexed for lookup.
#[derive(Debug, Default, Clone)]
pub struct AliasBook {
    counterparties: HashMap<String, String>,
    accounts: HashMap<Uuid, String>,
}

impl AliasBook {
    /// Builds the book from `(kind, key, alias)` triples. Rows with an
    /// unknown kind or a non-UUID account key are ignored.
    pub fn from_rows<'a>(rows: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>) -> Self {
        let mut book = AliasBook::default();
        for (kind, key, alias) in rows {
            match AliasKind::parse(kind) {
                Some(AliasKind::Counterparty) => {
                    book.counterparties.insert(key.to_string(), alias.to_string());
                }
                Some(AliasKind::Account) => {
                    if let Ok(id) = Uuid::parse_str(key) {
                        book.accounts.insert(id, alias.to_string());
                    }
                }
                None => {}
            }
        }
        book
    }

    /// The alias for the transaction's normalised key if there is one, else
    /// the bank's own counterparty string, untouched.
    pub fn counterparty_display(&self, key: Option<&str>, raw: Option<&str>) -> Option<String> {
        key.and_then(|k| self.counterparties.get(k))
            .cloned()
            .or_else(|| raw.map(str::to_string))
    }

    /// The alias for the account if there is one, else its raw name.
    pub fn account_display(&self, id: Uuid, raw_name: String) -> String {
        self.accounts.get(&id).cloned().unwrap_or(raw_name)
    }
}

/// The best name an account has without an alias: the login label, then the
/// IBAN, then the bank's display id, then the external id.
pub fn account_raw_name(
    label: Option<&str>,
    iban: Option<&str>,
    display_id: Option<&str>,
    external_id: &str,
) -> String {
    [label, iban, display_id]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or(external_id)
        .to_string()
}

/// The key the labeler indexes on for whatever the user typed or clicked: an
/// already-normalised key passes through, a raw name ("Amazon Payments
/// Europe S.C.A.") is normalised exactly like a transaction's counterparty.
pub fn canonical_counterparty_key(input: &str) -> async_graphql::Result<String> {
    let key = crate::labeling::normalize::normalize(Some(input), None);
    if key.is_empty() {
        return Err(validation_error("key must not be empty"));
    }
    Ok(key)
}

/// Trims and bounds a nickname.
pub fn clean_alias(input: &str) -> async_graphql::Result<String> {
    let alias = input.trim();
    if alias.is_empty() {
        return Err(validation_error("alias must not be empty; remove the alias instead"));
    }
    if alias.chars().count() > MAX_ALIAS_LEN {
        return Err(validation_error(format!("alias must be at most {MAX_ALIAS_LEN} characters")));
    }
    if alias.chars().any(char::is_control) {
        return Err(validation_error("alias must not contain control characters"));
    }
    Ok(alias.to_string())
}

/// Most frequent spelling per key, ties broken alphabetically so the answer
/// is stable. Also counts every row per key, spelled or not.
fn spellings_and_counts(
    rows: Vec<(Option<String>, Option<String>)>,
) -> HashMap<String, (String, i32)> {
    let mut names: HashMap<String, HashMap<String, i32>> = HashMap::new();
    let mut counts: HashMap<String, i32> = HashMap::new();
    for (key, name) in rows {
        let Some(key) = key else { continue };
        *counts.entry(key.clone()).or_default() += 1;
        if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
            *names.entry(key).or_default().entry(name).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .map(|(key, count)| {
            let name = names
                .get(&key)
                .and_then(|s| s.iter().max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0))))
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| key.clone());
            (key, (name, count))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Per-request resolution
// ---------------------------------------------------------------------------

/// Per-request cache (inserted by `request_with_auth`): the caller's
/// [`AliasBook`], loaded by one query the first time a display field
/// resolves. A page of N transactions and M accounts then costs one alias
/// query, not N + M.
#[derive(Default)]
pub struct AliasCache {
    book: OnceCell<Arc<AliasBook>>,
}

async fn load_book(db: &DatabaseConnection, user_id: Uuid) -> async_graphql::Result<AliasBook> {
    let rows = display_alias::Entity::find()
        .filter(display_alias::Column::UserId.eq(user_id))
        .all(db)
        .await?;
    Ok(AliasBook::from_rows(
        rows.iter().map(|r| (r.kind.as_str(), r.key.as_str(), r.alias.as_str())),
    ))
}

async fn book_for(ctx: &Context<'_>) -> async_graphql::Result<Arc<AliasBook>> {
    let user = current_user(ctx)?;
    let db: &Arc<DatabaseConnection> = ctx.data()?;
    match ctx.data::<AliasCache>() {
        Ok(cache) => cache
            .book
            .get_or_try_init(|| async { load_book(db, user.user_id).await.map(Arc::new) })
            .await
            .cloned(),
        // No per-request cache (e.g. a bare test schema): load for this call.
        Err(_) => Ok(Arc::new(load_book(db, user.user_id).await?)),
    }
}

/// `Transaction.counterpartyDisplayName`.
pub(crate) async fn counterparty_display_name(
    ctx: &Context<'_>,
    tx: &Transaction,
) -> async_graphql::Result<Option<String>> {
    let book = book_for(ctx).await?;
    Ok(book.counterparty_display(tx.counterparty_key.as_deref(), tx.counterparty_name.as_deref()))
}

#[ComplexObject]
impl Account {
    /// The caller's alias for this account if they set one, else the login
    /// label, IBAN or display id. `label` itself stays the login label.
    async fn display_name(&self, ctx: &Context<'_>) -> async_graphql::Result<String> {
        let book = book_for(ctx).await?;
        let raw = account_raw_name(
            self.label.as_deref(),
            self.iban.as_deref(),
            self.display_id.as_deref(),
            &self.external_id,
        );
        Ok(book.account_display(self.id.0, raw))
    }
}

// ---------------------------------------------------------------------------
// Query + mutations (thin delegates from queries.rs / mutations.rs)
// ---------------------------------------------------------------------------

/// `displayAliases`: the caller's aliases, most recently changed first, each
/// with the original name alongside.
pub(crate) async fn list(ctx: &Context<'_>) -> async_graphql::Result<Vec<DisplayAlias>> {
    let user = current_user(ctx)?;
    let db: &Arc<DatabaseConnection> = ctx.data()?;
    let rows = display_alias::Entity::find()
        .filter(display_alias::Column::UserId.eq(user.user_id))
        .order_by_desc(display_alias::Column::Revision)
        .all(db.as_ref())
        .await?;

    let keys: Vec<String> = rows
        .iter()
        .filter(|r| r.kind == AliasKind::Counterparty.as_str())
        .map(|r| r.key.clone())
        .collect();
    let evidence = counterparty_evidence(db.as_ref(), &user.account_ids, &keys).await?;
    let account_ids: Vec<Uuid> = rows
        .iter()
        .filter(|r| r.kind == AliasKind::Account.as_str())
        .filter_map(|r| Uuid::parse_str(&r.key).ok())
        .collect();
    let account_names = account_names(db.as_ref(), &user.account_ids, &account_ids).await?;

    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let kind = AliasKind::parse(&row.kind)?;
            let (raw_name, transaction_count) = match kind {
                AliasKind::Counterparty => {
                    let (name, count) = evidence
                        .get(&row.key)
                        .cloned()
                        .unwrap_or_else(|| (row.key.clone(), 0));
                    (name, Some(count))
                }
                AliasKind::Account => {
                    let name = Uuid::parse_str(&row.key)
                        .ok()
                        .and_then(|id| account_names.get(&id).cloned())
                        .unwrap_or_else(|| row.key.clone());
                    (name, None)
                }
            };
            Some(DisplayAlias {
                kind: kind.into(),
                key: row.key,
                alias: row.alias,
                raw_name,
                transaction_count,
                updated_at: GqlDateTime(row.revision.with_timezone(&Utc)),
            })
        })
        .collect())
}

async fn counterparty_evidence(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    keys: &[String],
) -> async_graphql::Result<HashMap<String, (String, i32)>> {
    if scoped_ids.is_empty() || keys.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = transaction::Entity::find()
        .select_only()
        .columns([transaction::Column::CounterpartyKey, transaction::Column::CounterpartyName])
        .filter(transaction::Column::AccountId.is_in(scoped_ids.to_vec()))
        .filter(transaction::Column::CounterpartyKey.is_in(keys.to_vec()))
        .into_tuple::<(Option<String>, Option<String>)>()
        .all(db)
        .await?;
    Ok(spellings_and_counts(rows))
}

async fn account_names(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    wanted: &[Uuid],
) -> async_graphql::Result<HashMap<Uuid, String>> {
    let ids: Vec<Uuid> = wanted.iter().copied().filter(|id| scoped_ids.contains(id)).collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = account::Entity::find().filter(account::Column::Id.is_in(ids)).all(db).await?;
    Ok(rows
        .into_iter()
        .map(|a| {
            let name = account_raw_name(
                a.label.as_deref(),
                a.iban.as_deref(),
                a.display_id.as_deref(),
                &a.external_id,
            );
            (a.id, name)
        })
        .collect())
}

/// Resolves what the user typed into the key the alias is stored under.
fn canonical_key(
    kind: AliasKind,
    input: &str,
    scoped_ids: &[Uuid],
) -> async_graphql::Result<String> {
    match kind {
        AliasKind::Counterparty => canonical_counterparty_key(input),
        AliasKind::Account => {
            let id = Uuid::parse_str(input.trim())
                .map_err(|_| validation_error("an account alias is keyed by the account id"))?;
            if !scoped_ids.contains(&id) {
                return Err(async_graphql::Error::new(format!("account '{id}' is not accessible")));
            }
            Ok(id.to_string())
        }
    }
}

fn publisher<'a>(ctx: &'a Context<'_>) -> Option<&'a Arc<EventPublisher>> {
    ctx.data::<Arc<EventPublisher>>().ok()
}

/// `setDisplayAlias`: publish the record, then project it. Setting again
/// replaces the previous alias.
pub(crate) async fn set(
    ctx: &Context<'_>,
    kind: GqlAliasKind,
    key: &str,
    alias: &str,
) -> async_graphql::Result<DisplayAlias> {
    let user = current_user(ctx)?;
    let db: &Arc<DatabaseConnection> = ctx.data()?;
    let kind = AliasKind::from(kind);
    let key = canonical_key(kind, key, &user.account_ids)?;
    let alias = clean_alias(alias)?;
    let publisher = publisher(ctx).ok_or_else(kafka_unavailable_error)?;

    let record = DisplayAliasRecord {
        schema_version: CURRENT_SCHEMA_VERSION,
        user_id: user.user_id,
        kind,
        key: key.clone(),
        alias,
        revision: Utc::now(),
    };
    let value = serde_json::to_vec(&record)
        .map_err(|e| async_graphql::Error::new(format!("failed to serialize display alias: {e}")))?;
    publish_event(publisher, TOPIC_DISPLAY_ALIAS, &record.topic_key(), &value).await?;
    proj::project_display_alias(db.as_ref(), user.user_id, kind, &key, Some(record)).await?;

    list(ctx)
        .await?
        .into_iter()
        .find(|a| AliasKind::from(a.kind) == kind && a.key == key)
        .ok_or_else(|| async_graphql::Error::new("display alias did not take effect"))
}

/// `removeDisplayAlias`: a tombstone, then the matching delete; the original
/// name shows again. True when an alias existed.
pub(crate) async fn remove(
    ctx: &Context<'_>,
    kind: GqlAliasKind,
    key: &str,
) -> async_graphql::Result<bool> {
    let user = current_user(ctx)?;
    let db: &Arc<DatabaseConnection> = ctx.data()?;
    let kind = AliasKind::from(kind);
    let key = canonical_key(kind, key, &user.account_ids)?;
    let publisher = publisher(ctx).ok_or_else(kafka_unavailable_error)?;

    let existed = display_alias::Entity::find_by_id((user.user_id, kind.as_str().to_string(), key.clone()))
        .one(db.as_ref())
        .await?
        .is_some();
    publish_tombstone(publisher, TOPIC_DISPLAY_ALIAS, &display_alias_topic_key(user.user_id, kind, &key))
        .await?;
    proj::project_display_alias(db.as_ref(), user.user_id, kind, &key, None).await?;
    Ok(existed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> AliasBook {
        let account = Uuid::from_bytes([7; 16]).to_string();
        AliasBook::from_rows([
            ("counterparty", "amazon", "Amazon (shopping)"),
            ("account", account.as_str(), "Joint account"),
            ("bogus", "x", "ignored"),
            ("account", "not-a-uuid", "ignored"),
        ])
    }

    #[test]
    fn an_unaliased_merchant_shows_the_bank_name() {
        assert_eq!(
            book().counterparty_display(Some("rewe"), Some("REWE Markt GmbH")),
            Some("REWE Markt GmbH".to_string())
        );
        // No key yet (not normalised) and no raw name at all.
        assert_eq!(book().counterparty_display(None, Some("Foo")), Some("Foo".to_string()));
        assert_eq!(book().counterparty_display(None, None), None);
    }

    #[test]
    fn an_alias_replaces_the_name_for_every_spelling_of_the_merchant() {
        let spellings = [
            "AMAZON PAYMENTS EUROPE S.C.A.",
            "Amazon Payments Europe S.C.A.",
            "amazon payments europe s.c.a.",
            "  Amazon Payments Europe S.C.A. 12.03  ",
        ];
        let keys: Vec<String> = spellings
            .iter()
            .map(|s| crate::labeling::normalize::normalize(Some(s), None))
            .collect();
        assert!(keys.windows(2).all(|w| w[0] == w[1]), "variants must share one key: {keys:?}");

        let book = AliasBook::from_rows([("counterparty", keys[0].as_str(), "Shopping")]);
        for (spelling, key) in spellings.iter().zip(&keys) {
            assert_eq!(
                book.counterparty_display(Some(key), Some(spelling)),
                Some("Shopping".to_string())
            );
        }
    }

    #[test]
    fn a_typed_name_and_a_stored_key_resolve_to_the_same_alias_key() {
        let typed = canonical_counterparty_key("  Amazon Payments GmbH ").unwrap();
        assert_eq!(canonical_counterparty_key(&typed).unwrap(), typed);
        assert!(canonical_counterparty_key("   ").is_err());
    }

    #[test]
    fn removing_the_alias_restores_the_original_name() {
        let with = AliasBook::from_rows([("counterparty", "amazon", "Shopping")]);
        let without = AliasBook::from_rows([]);
        assert_eq!(
            with.counterparty_display(Some("amazon"), Some("Amazon EU")),
            Some("Shopping".to_string())
        );
        assert_eq!(
            without.counterparty_display(Some("amazon"), Some("Amazon EU")),
            Some("Amazon EU".to_string())
        );
    }

    #[test]
    fn an_account_alias_falls_back_to_the_raw_name() {
        let aliased = Uuid::from_bytes([7; 16]);
        let other = Uuid::from_bytes([8; 16]);
        assert_eq!(book().account_display(aliased, "Pavlo".into()), "Joint account");
        assert_eq!(book().account_display(other, "Pavlo".into()), "Pavlo");
    }

    #[test]
    fn the_raw_account_name_prefers_label_then_iban_then_ids() {
        assert_eq!(account_raw_name(Some("Pavlo"), Some("DE1"), Some("9"), "ext"), "Pavlo");
        assert_eq!(account_raw_name(Some("  "), Some("DE1"), Some("9"), "ext"), "DE1");
        assert_eq!(account_raw_name(None, None, Some("9"), "ext"), "9");
        assert_eq!(account_raw_name(None, None, None, "ext"), "ext");
    }

    #[test]
    fn aliases_are_trimmed_and_bounded() {
        assert_eq!(clean_alias("  Mum ").unwrap(), "Mum");
        assert!(clean_alias("   ").is_err());
        assert!(clean_alias(&"x".repeat(MAX_ALIAS_LEN + 1)).is_err());
        assert!(clean_alias(&"x".repeat(MAX_ALIAS_LEN)).is_ok());
        assert!(clean_alias("a\nb").is_err());
    }

    #[test]
    fn an_account_alias_must_name_an_accessible_account() {
        let mine = Uuid::from_bytes([1; 16]);
        let theirs = Uuid::from_bytes([2; 16]);
        assert_eq!(
            canonical_key(AliasKind::Account, &mine.to_string(), &[mine]).unwrap(),
            mine.to_string()
        );
        assert!(canonical_key(AliasKind::Account, &theirs.to_string(), &[mine]).is_err());
        assert!(canonical_key(AliasKind::Account, "nope", &[mine]).is_err());
    }

    #[test]
    fn evidence_uses_the_most_frequent_spelling() {
        let rows = vec![
            (Some("amazon".to_string()), Some("AMAZON EU".to_string())),
            (Some("amazon".to_string()), Some("Amazon EU".to_string())),
            (Some("amazon".to_string()), Some("Amazon EU".to_string())),
            (Some("lidl".to_string()), None),
            (None, Some("ignored".to_string())),
        ];
        let evidence = spellings_and_counts(rows);
        assert_eq!(evidence["amazon"], ("Amazon EU".to_string(), 3));
        assert_eq!(evidence["lidl"], ("lidl".to_string(), 1));
        assert_eq!(evidence.len(), 2);
    }
}
