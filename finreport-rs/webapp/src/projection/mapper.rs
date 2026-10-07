//! The `SourceMapper` trait (§2.4) and the `(source, origin)` registry that
//! selects one for a given record.

use std::collections::HashMap;

use crate::kafka::envelope::SourceEvent;
use crate::projection::records::{AccountRecord, BalanceRecord, MapError, TransactionRecord};

/// Everything a [`SourceMapper`] needs to map one record.
///
/// Wraps the frozen (§2.2) [`SourceEvent`] — bytes + the headers every
/// consumer gets — with the two login-provenance headers `SourceEvent`
/// deliberately does not carry (`comdirect_account_key`/`_name`, predating
/// the envelope contract and specific to the Comdirect source). `account.label`
/// (the login's display name, §3) needs the latter, so `ComdirectMapper`
/// needs it too; adding it here rather than widening the frozen `SourceEvent`
/// keeps WP0's contract untouched (see the session's final report for why).
///
/// Still pure: no DB, no clock, no network — everything on this type comes
/// straight off the envelope the projector already parsed off the wire.
pub struct MapperInput<'a> {
    pub event: SourceEvent<'a>,
    pub comdirect_account_key: Option<&'a str>,
    pub comdirect_account_name: Option<&'a str>,
}

/// Maps bytes + headers for one source into the normalized record shapes
/// (§2.4). Implementations must be pure functions of their input — no DB, no
/// clock, no network — so they are unit-testable in isolation and so a
/// replay of the same bytes always produces the same row.
pub trait SourceMapper: Send + Sync {
    /// The `source` header value this mapper claims (diagnostic only; the
    /// registry key is what actually selects a mapper).
    fn source(&self) -> &'static str;
    fn map_account(&self, input: &MapperInput<'_>) -> Result<AccountRecord, MapError>;
    fn map_balance(&self, input: &MapperInput<'_>) -> Result<BalanceRecord, MapError>;
    fn map_transaction(&self, input: &MapperInput<'_>) -> Result<TransactionRecord, MapError>;
}

/// `(source, origin)` header pair -> mapper (§2.4). A pair with no
/// registered mapper is logged and every record carrying it is skipped,
/// exactly like a record that fails to parse.
pub struct MapperRegistry {
    mappers: HashMap<(String, String), Box<dyn SourceMapper>>,
}

impl MapperRegistry {
    pub fn new() -> Self {
        Self {
            mappers: HashMap::new(),
        }
    }

    /// The registry this iteration ships: `ComdirectMapper` for
    /// `(comdirect, source)`, `LegacyMapper` for `(comdirect, legacy-backfill)`
    /// (§2.4, §2.8).
    pub fn with_default_mappers() -> Self {
        use crate::kafka::envelope::{ORIGIN_LEGACY_BACKFILL, ORIGIN_SOURCE, SOURCE_COMDIRECT};
        use crate::projection::comdirect::ComdirectMapper;
        use crate::projection::legacy::LegacyMapper;

        let mut registry = Self::new();
        registry.register(SOURCE_COMDIRECT, ORIGIN_SOURCE, Box::new(ComdirectMapper));
        registry.register(
            SOURCE_COMDIRECT,
            ORIGIN_LEGACY_BACKFILL,
            Box::new(LegacyMapper),
        );
        registry
    }

    pub fn register(&mut self, source: &str, origin: &str, mapper: Box<dyn SourceMapper>) {
        self.mappers
            .insert((source.to_string(), origin.to_string()), mapper);
    }

    pub fn resolve(&self, source: &str, origin: &str) -> Option<&dyn SourceMapper> {
        self.mappers
            .get(&(source.to_string(), origin.to_string()))
            .map(|boxed| boxed.as_ref())
    }
}

impl Default for MapperRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kafka::envelope::{ORIGIN_LEGACY_BACKFILL, ORIGIN_SOURCE, SOURCE_COMDIRECT};

    #[test]
    fn default_registry_resolves_comdirect_source_and_legacy_backfill() {
        let registry = MapperRegistry::with_default_mappers();

        assert!(registry.resolve(SOURCE_COMDIRECT, ORIGIN_SOURCE).is_some());
        assert!(registry
            .resolve(SOURCE_COMDIRECT, ORIGIN_LEGACY_BACKFILL)
            .is_some());
    }

    #[test]
    fn unknown_source_or_origin_resolves_to_none() {
        let registry = MapperRegistry::with_default_mappers();

        assert!(registry.resolve("other-source", ORIGIN_SOURCE).is_none());
        assert!(registry
            .resolve(SOURCE_COMDIRECT, "some-other-origin")
            .is_none());
    }
}
