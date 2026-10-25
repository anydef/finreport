//! Parsed C24 row types.
//!
//! Nothing here is a wire contract yet: this slice is the parser only, and the
//! record shape published to Kafka is still an open decision. Treat these
//! structs as internal until that decision is made and written down.

use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// C24's own categorisation of a row (`Kategorie` / `Unterkategorie`).
///
/// **Evidence, never a decision.** A later slice may feed this to the labeler
/// as a *hint*, on a par with an LLM suggestion. It must never be allowed to
/// override a user override or a learned rule: the precedence chain stays
/// `user override > rule > LLM cache > LLM` (see `docs/specs/iteration-2.md`
/// §2.2–§2.9), and C24's guess enters at most as an input to the bottom of
/// that chain. It is also C24's taxonomy, not ours — mapping it onto
/// `prompts/taxonomy.json` is a separate problem, not done here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct C24Category {
    /// `Kategorie`, verbatim. Empty cell -> `None` on [`C24Row::category`].
    pub category: String,
    /// `Unterkategorie`, verbatim, when present.
    pub subcategory: Option<String>,
}

/// One data row of a C24 export, with its derived stable identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct C24Row {
    /// `Transaktionstyp`, verbatim.
    ///
    /// Deliberately a `String`, not an enum: an unknown type must never
    /// reject the file. Observed in the real export so far:
    /// `Kartenzahlung`, `Online-Kartenzahlung`, `Echtzeit-Überweisung`.
    pub transaction_type: String,
    /// `Buchungsdatum` (`dd.MM.yyyy`). The booking day, and the scope of the
    /// occurrence ordinal in [`C24Row::external_id`].
    pub booking_date: NaiveDate,
    /// `Karteneinsatz` (`dd.MM.yyyy HH:mm`): when the card was used. Empty
    /// for non-card rows such as transfers.
    pub card_used_at: Option<NaiveDateTime>,
    /// `Betrag`, parsed from the German-formatted, currency-suffixed cell.
    /// Negative for debits, positive for credits.
    pub amount: Decimal,
    /// `Zahlungsempfänger`: the counterparty as C24 labels it.
    pub counterparty: String,
    /// `IBAN`. Absent on card payments.
    pub iban: Option<String>,
    /// `BIC`. Absent on card payments.
    pub bic: Option<String>,
    /// `Verwendungszweck`: the payment purpose. Absent on card payments.
    pub purpose: Option<String>,
    /// `Beschreibung`: C24's own extra description. Frequently absent.
    pub description: Option<String>,
    /// `Kontonummer`: the 10-digit account number this row belongs to. One
    /// file can carry several accounts, so this is never assumed constant.
    pub account_number: String,
    /// `Kontoname`: that account's label, e.g. `C24 Smartkonto`.
    pub account_name: String,
    /// C24's own categorisation, when the row carries one. See
    /// [`C24Category`]: evidence, not a decision.
    pub category: Option<C24Category>,
    /// `Bargeldabhebung`, when the row has the trailing 14th field at all.
    pub cash_withdrawal: Option<String>,
    /// Hex SHA-256 digest over the identifying fields. Shared by genuinely
    /// duplicate rows; the ordinal tells them apart. The scheme and its
    /// accepted weakness are documented in the crate's `identity` module.
    pub content_key: String,
    /// 1-based occurrence of this `content_key` within its
    /// `(account_number, booking_date)` group.
    pub occurrence: u32,
    /// Stable id for this row, `"<content_key>-<occurrence>"`. Suitable as
    /// the `external_id` of `transaction_uuid(source, external_id)`.
    pub external_id: String,
}
