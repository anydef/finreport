//! The `finreport.cash-account` and `finreport.cash-transaction` topics:
//! accounts and rows the user creates and types in by hand (a wallet, a
//! cash tin), on topics of their own.
//!
//! # These two types are a wire contract
//!
//! [`CashAccountRecord`] and [`CashTransactionRecord`] are stored on
//! compacted, `prevent_destroy` topics, and their serde shape **is** the
//! stored payload. Renaming a field (or changing its type) silently breaks
//! every record already stored, so any such change needs a
//! [`CURRENT_SCHEMA_VERSION`] bump and a migration path, not just a passing
//! test suite. The `*_wire_shape_is_frozen` tests below exist to make that
//! impossible to do by accident. (`amazon_orders::Order` being the payload
//! of `finreport.purchase-order` is the trap this avoids repeating.)
//!
//! # User-authored, not bank-sourced
//!
//! Every other `account` / `transaction` row is projected from something a
//! bank said. A cash account is the user's own fact, so the log has to be
//! enough to rebuild it completely: [`CashAccountRecord`] therefore carries
//! its owner's `user_id` and the projection links `user_account` from it
//! (a C24 account, by contrast, is linked by the upload mutation, because it
//! comes from a bank statement). `just dev-reset` keeps `user_account`, but a
//! lost database must not lose the user's own accounts.
//!
//! # Identity
//!
//! `external_id` is a **generated UUID**, never the account's name: the
//! account is the user's own creation, so nothing about it is derivable, and
//! two users may each have a "Wallet" and they must be different accounts.
//! This slice treats it as an opaque string; the mutation (a later slice)
//! generates it.
//!
//! # No balance topic
//!
//! There is deliberately **no** `cash-account-balance` topic. No bank
//! reports a balance for a wallet; it is the running sum of the account's
//! own transactions, derived on read.
//!
//! # Deletion is a tombstone
//!
//! A null-payload record on either topic deletes the row. Deleting an
//! account removes every transaction it owns, and deleting a transaction
//! (or an account) clears any transfer pairing a bank-side row had with it;
//! see `projection::cash`.
//!
//! # Keys and headers
//!
//! Each topic is keyed by the entity's `external_id`. Headers use
//! `source = "cash"`, `origin = "user"`, `SourceContext::Unattributed`, and
//! the transaction topic carries the account's `external_id` as
//! `source_account_id` (also repeated in the payload, where the mapper checks
//! the two agree).

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Keyed by the account's generated `external_id`. Compacted, 1 partition,
/// `prevent_destroy`.
pub const TOPIC_CASH_ACCOUNT: &str = "finreport.cash-account";
/// Keyed by the transaction's generated `external_id`. Compacted, 1
/// partition, `prevent_destroy`.
pub const TOPIC_CASH_TRANSACTION: &str = "finreport.cash-transaction";

/// The `source` header / column value for cash accounts (lowercase, per §2.2).
pub const SOURCE_CASH: &str = "cash";

/// Payload schema version for both record types. Bump on any change to their
/// serde shape.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// `booking_status` of every cash row: the user types in what already
/// happened. The same literal the other sources store for a booked row.
pub const BOOKING_STATUS_BOOKED: &str = "BOOKED";

/// Published on [`TOPIC_CASH_ACCOUNT`]. Wire contract, see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CashAccountRecord {
    pub schema_version: u32,
    /// Generated UUID, opaque to this slice.
    pub external_id: String,
    /// The owning user. The projection links `user_account` from it.
    pub user_id: Uuid,
    /// The user's name for the account, e.g. `Wallet`.
    pub label: String,
    pub currency: String,
}

impl CashAccountRecord {
    pub fn topic_key(&self) -> &str {
        &self.external_id
    }
}

/// Published on [`TOPIC_CASH_TRANSACTION`]. Wire contract, see the module
/// docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CashTransactionRecord {
    pub schema_version: u32,
    /// Generated UUID, opaque to this slice.
    pub external_id: String,
    /// The owning account's `external_id`.
    pub account_external_id: String,
    pub booking_date: NaiveDate,
    #[serde(with = "rust_decimal::serde::str")]
    pub amount: Decimal,
    pub currency: String,
    /// Who was paid / who paid, if the user said.
    pub counterparty_name: Option<String>,
    pub description: Option<String>,
}

impl CashTransactionRecord {
    pub fn topic_key(&self) -> &str {
        &self.external_id
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub const USER_ID: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);
    pub const ACCOUNT_ID: &str = "7a1c0000-0000-4000-8000-000000000001";

    pub fn account() -> CashAccountRecord {
        CashAccountRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            external_id: ACCOUNT_ID.to_string(),
            user_id: USER_ID,
            label: "Wallet".to_string(),
            currency: "EUR".to_string(),
        }
    }

    pub fn transaction() -> CashTransactionRecord {
        CashTransactionRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            external_id: "7a1c0000-0000-4000-8000-0000000000a1".to_string(),
            account_external_id: ACCOUNT_ID.to_string(),
            booking_date: NaiveDate::from_ymd_opt(2026, 3, 4).unwrap(),
            amount: Decimal::new(-450, 2),
            currency: "EUR".to_string(),
            counterparty_name: Some("Beispiel Baeckerei".to_string()),
            description: Some("Broetchen".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn records_round_trip_through_json() {
        let tx = transaction();
        let back: CashTransactionRecord =
            serde_json::from_slice(&serde_json::to_vec(&tx).unwrap()).unwrap();
        assert_eq!(tx, back);
        let acct = account();
        let back: CashAccountRecord =
            serde_json::from_slice(&serde_json::to_vec(&acct).unwrap()).unwrap();
        assert_eq!(acct, back);
    }

    #[test]
    fn topic_key_is_the_external_id() {
        assert_eq!(account().topic_key(), ACCOUNT_ID);
        assert_eq!(transaction().topic_key(), transaction().external_id);
    }

    /// Freezes the stored shape. If this fails, you are changing the wire
    /// contract: bump `CURRENT_SCHEMA_VERSION` and add a migration path.
    #[test]
    fn transaction_wire_shape_is_frozen() {
        let json = serde_json::to_value(transaction()).unwrap();
        let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "account_external_id",
                "amount",
                "booking_date",
                "counterparty_name",
                "currency",
                "description",
                "external_id",
                "schema_version"
            ]
        );
        assert_eq!(json["amount"], "-4.50");
    }

    #[test]
    fn account_wire_shape_is_frozen() {
        let json = serde_json::to_value(account()).unwrap();
        let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["currency", "external_id", "label", "schema_version", "user_id"]
        );
    }
}
