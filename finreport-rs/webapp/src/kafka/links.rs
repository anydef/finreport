//! The user's `finreport.transaction-link` topic: a link is a human
//! declaration that some transactions offset others (today: a reimbursement
//! repaying an expense). Keyed by the link's own id, whole-state,
//! last-writer-wins - the same shape as `finreport.goal` and
//! `finreport.rule`, and for the same reason: a human decision belongs on a
//! compacted topic so a replay from offset 0 reproduces it.
//!
//! Deliberately **not** `finreport.transaction-insight`: that topic is the
//! detector's, keyed per transaction, and the detector rewrites it. A link is
//! declared by the user, may join any number of transactions, and no detector
//! may ever overwrite it. Keeping it on its own topic is what guarantees the
//! last part.
//!
//! No amounts are carried. The offset is derived from the members' own
//! amounts at read time (`crate::links`), so a link cannot go stale.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Keyed by the link's own UUID as a string. Compacted, partitions 1, RF 1,
/// `prevent_destroy`, iteration-1 envelope headers with `origin = user`; a
/// tombstone deletes the link and thereby restores the unlinked view exactly.
pub const TOPIC_TRANSACTION_LINK: &str = "finreport.transaction-link";

/// Current payload schema version for [`TransactionLinkRecord`].
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// What the link means. Open for extension: a new kind is a new variant
/// here and a sign rule in `links::sign_rule`; the table stores the
/// snake_case name as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// Money paid out, later paid back (wholly or in part) by someone else.
    Reimbursement,
}

impl LinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Reimbursement => "reimbursement",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reimbursement" => Some(LinkKind::Reimbursement),
            _ => None,
        }
    }
}

/// Which side of the link a member is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkRole {
    /// The side being offset (the bill that was paid).
    Expense,
    /// The side doing the offsetting (the money coming back).
    Offset,
}

impl LinkRole {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkRole::Expense => "expense",
            LinkRole::Offset => "offset",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "expense" => Some(LinkRole::Expense),
            "offset" => Some(LinkRole::Offset),
            _ => None,
        }
    }
}

/// One member, addressed the way `finreport.user-label` addresses a
/// transaction, so the record is self-describing across replays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkMemberRef {
    pub source: String,
    pub external_id: String,
    pub role: LinkRole,
}

/// Published on [`TOPIC_TRANSACTION_LINK`]. Whole-state: an edit republishes
/// the complete member list with a fresh `revision`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionLinkRecord {
    pub schema_version: u32,
    pub id: Uuid,
    pub kind: LinkKind,
    /// The user who made the declaration (audit only).
    pub owner_user_id: Option<Uuid>,
    /// At least one `expense` and one `offset`; any number of each.
    pub members: Vec<LinkMemberRef>,
    pub note: Option<String>,
    /// RFC 3339; last-writer-wins.
    pub revision: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trips_and_uses_snake_case_names() {
        let record = TransactionLinkRecord {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: Uuid::nil(),
            kind: LinkKind::Reimbursement,
            owner_user_id: None,
            members: vec![
                LinkMemberRef { source: "comdirect".into(), external_id: "a".into(), role: LinkRole::Expense },
                LinkMemberRef { source: "comdirect".into(), external_id: "b".into(), role: LinkRole::Offset },
            ],
            note: Some("dentist".into()),
            revision: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains(r#""kind":"reimbursement""#));
        assert!(json.contains(r#""role":"offset""#));
        assert_eq!(serde_json::from_str::<TransactionLinkRecord>(&json).unwrap(), record);
    }

    #[test]
    fn kind_and_role_names_round_trip_through_their_text_form() {
        assert_eq!(LinkKind::parse(LinkKind::Reimbursement.as_str()), Some(LinkKind::Reimbursement));
        for role in [LinkRole::Expense, LinkRole::Offset] {
            assert_eq!(LinkRole::parse(role.as_str()), Some(role));
        }
        assert_eq!(LinkKind::parse("nope"), None);
    }
}
