//! Stable identity for C24 rows.
//!
//! The export has **no transaction id column**, so one has to be derived. A
//! plain content hash is not enough: the real export contains byte-identical
//! rows that are genuinely distinct transactions (two identical card payments
//! to the same payee, with the same `Karteneinsatz` timestamp, and in another
//! case three of them). Hashing alone would collapse three transactions into
//! one and silently lose two.
//!
//! The scheme:
//!
//! ```text
//! content_key = SHA-256 over (kontonummer, buchungsdatum, karteneinsatz,
//!                             betrag, zahlungsempfaenger, iban,
//!                             verwendungszweck, beschreibung, transaktionstyp)
//! external_id = "<content_key>-<occurrence ordinal>"
//! ```
//!
//! **The ordinal is scoped to `(kontonummer, buchungsdatum, content_key)` —
//! the booking day, not the file.** That is what makes overlapping exports
//! agree. C24 filters by whole days, so any export containing a day contains
//! all of that day's rows; three identical rows on one day therefore get
//! ordinals 1, 2, 3 in *every* export that includes that day. Scoping the
//! ordinal to the file instead would shift it whenever the requested range
//! changed, and every re-import with a different range would duplicate.
//!
//! Deliberately **not** part of the key: `Kategorie`/`Unterkategorie` (C24's
//! own guess, which it may revise), `Kontoname` (a label) and
//! `Bargeldabhebung`. A row whose only change is one of those keeps its id.
//!
//! # Accepted weakness
//!
//! **If C24 ever re-words a row's `Verwendungszweck`, `Beschreibung` or
//! `Zahlungsempfänger`, that row re-imports as a new transaction.** The old id
//! no longer matches anything in the new export and the new id has never been
//! seen, so the user sees a duplicate. This is a known, accepted consequence
//! of having no id column: the alternative — leaving the text out of the key —
//! would merge distinct transactions that differ only by purpose, which is
//! worse. Documented here rather than hidden.

use sha2::{Digest, Sha256};

use crate::model::C24Row;

/// Separator for key parts. Joining with `\0` means `("ab", "c")` cannot
/// collide with `("a", "bc")` — the same discipline as
/// `webapp::kafka::envelope::uuid_v5_from_parts`. CSV cells do not contain
/// NUL in practice; if one ever did, that row's key would be ambiguous.
const SEP: u8 = 0;

/// Domain tag, so these digests can never be confused with another scheme's
/// over the same inputs.
const KIND: &str = "c24-transaction";

/// Hex SHA-256 over the identifying fields of `row`, ignoring the already
/// derived identity fields. A pure function of the row's content.
pub(crate) fn content_key(row: &C24Row) -> String {
    let card = row
        .card_used_at
        .map(|t| t.format("%Y-%m-%dT%H:%M").to_string())
        .unwrap_or_default();
    let booking = row.booking_date.format("%Y-%m-%d").to_string();
    // `normalize()` so "-1,00 €" and a hypothetical "-1 €" hash alike:
    // trailing-zero scale is formatting, not identity.
    let amount = row.amount.normalize().to_string();
    let parts: [&str; 9] = [
        &row.account_number,
        &booking,
        &card,
        &amount,
        &row.counterparty,
        row.iban.as_deref().unwrap_or(""),
        row.purpose.as_deref().unwrap_or(""),
        row.description.as_deref().unwrap_or(""),
        &row.transaction_type,
    ];

    let mut hasher = Sha256::new();
    hasher.update(KIND.as_bytes());
    for part in parts {
        hasher.update([SEP]);
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Assign `content_key`, `occurrence` and `external_id` to every row.
///
/// Pure and **independent of the order the rows arrive in**: rows are first
/// put into a canonical total order, then ordinals are handed out 1..n within
/// each `(account_number, booking_date, content_key)` group. The returned
/// `Vec` is in that canonical order too, so shuffling the input produces an
/// identical result, not merely identical ids.
pub(crate) fn assign_ids(mut rows: Vec<C24Row>) -> Vec<C24Row> {
    for row in &mut rows {
        row.content_key = content_key(row);
    }

    // Rows sharing a group are identical on every identifying field, so the
    // sort needs a tiebreak over the *non*-identifying ones to stay stable
    // under a shuffle. `tiebreak` supplies it; two rows identical in all 14
    // fields are interchangeable and either order gives the same id set.
    rows.sort_by(|a, b| {
        group_key(a)
            .cmp(&group_key(b))
            .then_with(|| tiebreak(a).cmp(&tiebreak(b)))
    });

    let mut occurrence = 0u32;
    let mut previous: Option<(String, String)> = None;
    for row in &mut rows {
        let key = (row.account_number.clone(), row.content_key.clone());
        // `group_key` includes the booking date, but the content key already
        // pins it, so account + content key identify the group.
        occurrence = if previous.as_ref() == Some(&key) {
            occurrence + 1
        } else {
            1
        };
        previous = Some(key);
        row.occurrence = occurrence;
        row.external_id = format!("{}-{}", row.content_key, occurrence);
    }
    rows
}

/// The ordinal's scope: account, booking day, content.
fn group_key(row: &C24Row) -> (&str, chrono::NaiveDate, &str) {
    (&row.account_number, row.booking_date, &row.content_key)
}

/// Fields outside the content key, so a shuffle cannot reorder two rows of
/// one group that differ only in these.
fn tiebreak(row: &C24Row) -> (&str, Option<&str>, Option<&str>, Option<&str>) {
    (
        &row.account_name,
        row.category.as_ref().map(|c| c.category.as_str()),
        row.category.as_ref().and_then(|c| c.subcategory.as_deref()),
        row.cash_withdrawal.as_deref(),
    )
}
