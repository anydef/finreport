//! The user's `finreport.account-savings` topic: which of their accounts they
//! treat as savings. A reporting choice, not a fact from the bank, so it is
//! per user - the owner is part of the key - exactly like a display nickname.
//!
//! It has its own topic rather than a `kind` on `finreport.display-alias`:
//! an alias is a *name* (`alias` is a required text payload), and a flag has
//! no payload. Presence is the flag; a tombstone clears it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Keyed `<user_id>:<account_id>` ([`account_savings_topic_key`]). Compacted,
/// partitions 1, RF 1, `prevent_destroy`; a tombstone clears the flag.
pub const TOPIC_ACCOUNT_SAVINGS: &str = "finreport.account-savings";

/// Current payload schema version for [`AccountSavingsRecord`].
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// `<user_id>:<account_id>`.
pub fn account_savings_topic_key(user_id: Uuid, account_id: Uuid) -> String {
    format!("{user_id}:{account_id}")
}

/// Inverse of [`account_savings_topic_key`]; `None` for anything malformed.
pub fn parse_account_savings_topic_key(topic_key: &str) -> Option<(Uuid, Uuid)> {
    let (user, account) = topic_key.split_once(':')?;
    Some((Uuid::parse_str(user).ok()?, Uuid::parse_str(account).ok()?))
}

/// Published on [`TOPIC_ACCOUNT_SAVINGS`] when the user marks an account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountSavingsRecord {
    pub schema_version: u32,
    pub user_id: Uuid,
    pub account_id: Uuid,
    /// RFC 3339; last-writer-wins (§2.1).
    pub revision: DateTime<Utc>,
}

impl AccountSavingsRecord {
    pub fn topic_key(&self) -> String {
        account_savings_topic_key(self.user_id, self.account_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_topic_key_round_trip() {
        let record = AccountSavingsRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            user_id: Uuid::from_bytes([1; 16]),
            account_id: Uuid::from_bytes([2; 16]),
            revision: Utc::now(),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(record, serde_json::from_str::<AccountSavingsRecord>(&json).unwrap());
        assert_eq!(
            parse_account_savings_topic_key(&record.topic_key()),
            Some((record.user_id, record.account_id))
        );
        assert!(parse_account_savings_topic_key("nope").is_none());
        assert!(parse_account_savings_topic_key(&format!("{}:x", record.user_id)).is_none());
    }
}
