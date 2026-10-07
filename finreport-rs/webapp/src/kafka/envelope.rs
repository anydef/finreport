//! The Kafka envelope contract (§2.2): header names, topic constants, the
//! write-side [`RecordMeta`] and read-side [`Envelope`], and the deterministic
//! id helpers the projector (§2.3) uses so a rebuild reproduces the same
//! `UUID`s every time.
//!
//! Frozen by WP0. Consumed by WP1 (producer side) and WP3 (projector side);
//! neither may change the header contract unilaterally — see `docs/specs/iteration-1.md` §2.2.

use chrono::{DateTime, Utc};
use rdkafka::message::{Header, Headers, OwnedHeaders};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Topics (§2.1). Reused verbatim from phase 1 — renaming any of these is a
// Kafka topic replace, and `terraform/kafka` marks them `prevent_destroy`.
// ---------------------------------------------------------------------------

/// Account entity snapshots. Compacted: the latest record per account wins.
pub const TOPIC_ACCOUNT: &str = "finreport.account";
/// Balance observations, one per account per import. Retained forever.
pub const TOPIC_ACCOUNT_BALANCE: &str = "finreport.account-balance";
/// Transaction events. Retained forever.
pub const TOPIC_TRANSACTION: &str = "finreport.transaction";
/// Per-account import resume points. Compacted. Importer-private: the
/// projector never consumes it (§2.3).
pub const TOPIC_IMPORT_WATERMARK: &str = "finreport.import-watermark";

// ---------------------------------------------------------------------------
// Header names (§2.2)
// ---------------------------------------------------------------------------

/// Stable source id, lowercase (e.g. `comdirect`). Required.
pub const HEADER_SOURCE: &str = "source";
/// The source's own account id. Required on every record but `account`
/// (whose payload already names it).
pub const HEADER_SOURCE_ACCOUNT_ID: &str = "source_account_id";
/// `source` (fetched from the provider) or `legacy-backfill` (§2.8). Required.
pub const HEADER_ORIGIN: &str = "origin";
/// Header contract version. Bumped only if the header contract itself
/// changes, never for payload/schema changes. Required.
pub const HEADER_SCHEMA_VERSION: &str = "schema_version";
/// RFC 3339, when the importer fetched the record. Required.
pub const HEADER_IMPORTED_AT: &str = "imported_at";
/// Config key of the importing login (`0`, `1`, ...). Predates the envelope.
pub const HEADER_COMDIRECT_ACCOUNT_KEY: &str = "comdirect_account_key";
/// That login's human-readable label, when configured. Predates the
/// envelope, optional.
pub const HEADER_COMDIRECT_ACCOUNT_NAME: &str = "comdirect_account_name";

// ---------------------------------------------------------------------------
// Header values
// ---------------------------------------------------------------------------

/// The only source this iteration knows. Lowercase per §2.2.
pub const SOURCE_COMDIRECT: &str = "comdirect";
/// `origin` header value: fetched live from the provider.
pub const ORIGIN_SOURCE: &str = "source";
/// `origin` header value: reconstructed from the renamed `legacy_*` tables
/// (§2.8). Deliberately hyphenated, unlike the `legacy` *column* value it
/// produces (§2.8 "spelling difference is deliberate").
pub const ORIGIN_LEGACY_BACKFILL: &str = "legacy-backfill";

/// Current header-contract version. Bump only when the headers themselves
/// change shape, not for payload/schema changes.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Namespace for every projector-owned deterministic id (§2.3). Random, fixed
/// once, never regenerated — changing it would re-id every row on the next
/// rebuild and sever every `user_account` link.
pub const FINREPORT_NS: Uuid = Uuid::from_bytes([
    0x1f, 0x6a, 0x6e, 0x2b, 0x9b, 0x3d, 0x4c, 0x8a, 0xab, 0x9e, 0x5a, 0x2b, 0x0c, 0x7d, 0x4e, 0x11,
]);

/// Deterministic id for an `account` row: `UUIDv5(FINREPORT_NS, source + external_id)` (§2.3).
pub fn account_uuid(source: &str, external_id: &str) -> Uuid {
    uuid_v5_from_parts("account", &[source, external_id])
}

/// Deterministic id for a `transaction` row: `UUIDv5(FINREPORT_NS, source + external_id)` (§2.3).
pub fn transaction_uuid(source: &str, external_id: &str) -> Uuid {
    uuid_v5_from_parts("transaction", &[source, external_id])
}

/// Deterministic id for an `account_balance` row:
/// `UUIDv5(FINREPORT_NS, account_id + balance_date)` (§2.3).
pub fn balance_uuid(account_id: Uuid, balance_date: chrono::NaiveDate) -> Uuid {
    uuid_v5_from_parts(
        "account_balance",
        &[
            &account_id.to_string(),
            &balance_date.format("%Y-%m-%d").to_string(),
        ],
    )
}

/// Joins `parts` with a `\0` separator (so `("ab", "c")` cannot collide with
/// `("a", "bc")`), prefixed with `kind` so the same string inputs never
/// collide across entity kinds (account vs. transaction vs. balance ids all
/// draw from the same namespace), and hashes the result as a v5 name.
fn uuid_v5_from_parts(kind: &str, parts: &[&str]) -> Uuid {
    let mut name = kind.to_string();
    for part in parts {
        name.push('\0');
        name.push_str(part);
    }
    Uuid::new_v5(&FINREPORT_NS, name.as_bytes())
}

// ---------------------------------------------------------------------------
// Write side
// ---------------------------------------------------------------------------

/// Full §2.2 header set for a record about to be published. All fields
/// required by the contract; `source_account_id` is `None` only for the
/// `account` topic, whose payload already names its own id.
pub struct RecordMeta<'a> {
    pub source: &'a str,
    pub source_account_id: Option<&'a str>,
    pub origin: &'a str,
    pub schema_version: u32,
    /// RFC 3339.
    pub imported_at: &'a str,
    pub comdirect_account_key: &'a str,
    pub comdirect_account_name: Option<&'a str>,
}

impl<'a> RecordMeta<'a> {
    /// Builds the Kafka headers for this record, per §2.2.
    pub fn headers(&self) -> OwnedHeaders {
        let schema_version = self.schema_version.to_string();
        let mut headers = OwnedHeaders::new()
            .insert(Header {
                key: HEADER_SOURCE,
                value: Some(self.source),
            })
            .insert(Header {
                key: HEADER_ORIGIN,
                value: Some(self.origin),
            })
            .insert(Header {
                key: HEADER_SCHEMA_VERSION,
                value: Some(schema_version.as_str()),
            })
            .insert(Header {
                key: HEADER_IMPORTED_AT,
                value: Some(self.imported_at),
            })
            .insert(Header {
                key: HEADER_COMDIRECT_ACCOUNT_KEY,
                value: Some(self.comdirect_account_key),
            });

        if let Some(source_account_id) = self.source_account_id {
            headers = headers.insert(Header {
                key: HEADER_SOURCE_ACCOUNT_ID,
                value: Some(source_account_id),
            });
        }
        if let Some(name) = self.comdirect_account_name {
            headers = headers.insert(Header {
                key: HEADER_COMDIRECT_ACCOUNT_NAME,
                value: Some(name),
            });
        }

        headers
    }
}

// ---------------------------------------------------------------------------
// Read side
// ---------------------------------------------------------------------------

/// A parsed, defaulted header set (§2.2). Phase-1 records predate most of
/// these headers, so every field here has a value even when the record on the
/// wire carried none of them — `parse` applies the documented defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub source: String,
    pub source_account_id: Option<String>,
    pub origin: String,
    pub schema_version: u32,
    pub imported_at: DateTime<Utc>,
    pub comdirect_account_key: Option<String>,
    pub comdirect_account_name: Option<String>,
}

impl Envelope {
    /// Parses `headers`, applying the §2.2 defaults for anything a phase-1
    /// record predates: `source` ⇒ `comdirect`, `origin` ⇒ `source`,
    /// `schema_version` ⇒ `1`. `imported_at`, if absent, falls back to
    /// `message_timestamp` (the record's own Kafka timestamp), which is what
    /// keeps replay deterministic (the log, not wall-clock `now()`).
    pub fn parse<H: Headers>(headers: Option<&H>, message_timestamp: DateTime<Utc>) -> Self {
        let get = |key: &str| -> Option<String> {
            let headers = headers?;
            for idx in 0..headers.count() {
                let header = headers.get(idx);
                if header.key == key {
                    return header
                        .value
                        .map(|v| String::from_utf8_lossy(v).into_owned());
                }
            }
            None
        };

        let imported_at = get(HEADER_IMPORTED_AT)
            .and_then(|v| DateTime::parse_from_rfc3339(&v).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or(message_timestamp);

        let schema_version = get(HEADER_SCHEMA_VERSION)
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);

        Envelope {
            source: get(HEADER_SOURCE).unwrap_or_else(|| SOURCE_COMDIRECT.to_string()),
            source_account_id: get(HEADER_SOURCE_ACCOUNT_ID),
            origin: get(HEADER_ORIGIN).unwrap_or_else(|| ORIGIN_SOURCE.to_string()),
            schema_version,
            imported_at,
            comdirect_account_key: get(HEADER_COMDIRECT_ACCOUNT_KEY),
            comdirect_account_name: get(HEADER_COMDIRECT_ACCOUNT_NAME),
        }
    }
}

/// Whether a record's value is the bank's own bytes, byte for byte, rather
/// than our own reconstruction (§2.8). `origin == "source"` is the only case
/// that is bank-verbatim; `legacy-backfill` (and any future producer origin)
/// is not.
pub fn is_bank_verbatim(envelope: &Envelope) -> bool {
    envelope.origin == ORIGIN_SOURCE
}

/// A single decoded ingest record, ready for a [`SourceMapper`] (§2.4). Mapper
/// inputs are pure — bytes and headers only, no DB/clock/network — which is
/// what makes mapping unit-testable and replay-deterministic.
pub struct SourceEvent<'a> {
    pub source: &'a str,
    pub source_account_id: Option<&'a str>,
    pub key: &'a str,
    pub payload: &'a [u8],
    pub imported_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdkafka::message::OwnedHeaders;

    fn headers_with(pairs: &[(&str, &str)]) -> OwnedHeaders {
        let mut headers = OwnedHeaders::new();
        for (key, value) in pairs {
            headers = headers.insert(Header {
                key,
                value: Some(*value),
            });
        }
        headers
    }

    #[test]
    fn headerless_record_gets_phase_1_defaults() {
        let message_timestamp = DateTime::parse_from_rfc3339("2024-05-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let envelope = Envelope::parse(None::<&OwnedHeaders>, message_timestamp);

        assert_eq!(envelope.source, SOURCE_COMDIRECT);
        assert_eq!(envelope.origin, ORIGIN_SOURCE);
        assert_eq!(envelope.schema_version, 1);
        assert_eq!(envelope.imported_at, message_timestamp);
        assert_eq!(envelope.source_account_id, None);
        assert!(is_bank_verbatim(&envelope));
    }

    #[test]
    fn phase_1_headers_only_still_default_the_rest() {
        // What `producer.rs` actually set before this contract existed.
        let headers = headers_with(&[
            (HEADER_COMDIRECT_ACCOUNT_KEY, "0"),
            (HEADER_COMDIRECT_ACCOUNT_NAME, "Main"),
            (HEADER_IMPORTED_AT, "2024-05-01T12:00:00Z"),
        ]);
        let message_timestamp = Utc::now();

        let envelope = Envelope::parse(Some(&headers), message_timestamp);

        assert_eq!(envelope.source, SOURCE_COMDIRECT);
        assert_eq!(envelope.origin, ORIGIN_SOURCE);
        assert_eq!(envelope.schema_version, 1);
        assert_eq!(
            envelope.comdirect_account_key.as_deref(),
            Some("0")
        );
        assert_eq!(
            envelope.comdirect_account_name.as_deref(),
            Some("Main")
        );
        assert_eq!(
            envelope.imported_at,
            DateTime::parse_from_rfc3339("2024-05-01T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn full_envelope_round_trips_through_record_meta() {
        let meta = RecordMeta {
            source: SOURCE_COMDIRECT,
            source_account_id: Some("DE00-ACC-1"),
            origin: ORIGIN_SOURCE,
            schema_version: CURRENT_SCHEMA_VERSION,
            imported_at: "2024-06-15T08:30:00Z",
            comdirect_account_key: "1",
            comdirect_account_name: Some("Joint"),
        };

        let headers = meta.headers();
        let envelope = Envelope::parse(Some(&headers), Utc::now());

        assert_eq!(envelope.source, SOURCE_COMDIRECT);
        assert_eq!(envelope.source_account_id.as_deref(), Some("DE00-ACC-1"));
        assert_eq!(envelope.origin, ORIGIN_SOURCE);
        assert_eq!(envelope.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(
            envelope.imported_at,
            DateTime::parse_from_rfc3339("2024-06-15T08:30:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
        assert_eq!(envelope.comdirect_account_key.as_deref(), Some("1"));
        assert_eq!(envelope.comdirect_account_name.as_deref(), Some("Joint"));
    }

    #[test]
    fn record_meta_without_optional_fields_omits_their_headers() {
        let meta = RecordMeta {
            source: SOURCE_COMDIRECT,
            // Headerless on the `account` topic: the payload already names
            // its own id (§2.2).
            source_account_id: None,
            origin: ORIGIN_SOURCE,
            schema_version: CURRENT_SCHEMA_VERSION,
            imported_at: "2024-06-15T08:30:00Z",
            comdirect_account_key: "1",
            comdirect_account_name: None,
        };

        let headers = meta.headers();
        let envelope = Envelope::parse(Some(&headers), Utc::now());

        assert_eq!(envelope.source_account_id, None);
        assert_eq!(envelope.comdirect_account_name, None);
    }

    #[test]
    fn legacy_backfill_origin_is_not_bank_verbatim() {
        let headers = headers_with(&[(HEADER_ORIGIN, ORIGIN_LEGACY_BACKFILL)]);
        let envelope = Envelope::parse(Some(&headers), Utc::now());

        assert_eq!(envelope.origin, ORIGIN_LEGACY_BACKFILL);
        assert!(!is_bank_verbatim(&envelope));
    }

    #[test]
    fn deterministic_ids_are_stable_and_source_scoped() {
        let a1 = account_uuid(SOURCE_COMDIRECT, "ACC-1");
        let a2 = account_uuid(SOURCE_COMDIRECT, "ACC-1");
        assert_eq!(a1, a2, "same (source, external_id) must reproduce the same id");

        let other_source = account_uuid("other-source", "ACC-1");
        assert_ne!(a1, other_source, "ids must not collide across sources");

        let t = transaction_uuid(SOURCE_COMDIRECT, "ACC-1");
        assert_ne!(
            a1, t,
            "account and transaction ids must not collide even for the same string inputs"
        );
    }

    #[test]
    fn balance_ids_are_scoped_by_account_and_date() {
        let account_id = account_uuid(SOURCE_COMDIRECT, "ACC-1");
        let date = chrono::NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        let other_date = chrono::NaiveDate::from_ymd_opt(2024, 6, 16).unwrap();

        let b1 = balance_uuid(account_id, date);
        let b2 = balance_uuid(account_id, date);
        assert_eq!(b1, b2);

        let b3 = balance_uuid(account_id, other_date);
        assert_ne!(b1, b3, "different dates must not collide");
    }
}

