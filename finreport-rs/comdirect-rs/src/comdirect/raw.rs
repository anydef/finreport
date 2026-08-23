use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

/// Pairs a parsed record with the exact raw JSON it was parsed from.
///
/// The Kafka publishing path needs to forward Comdirect's JSON byte-for-byte,
/// but the importer still wants typed access to the same record. Capturing
/// `raw` via `serde_json::value::RawValue` at parse time (rather than
/// re-serializing `parsed` afterwards with `serde_json::to_string`) is what
/// makes that safe: a round-trip through our structs would silently drop any
/// field we don't model and could reformat/reorder what's left.
#[derive(Debug)]
pub struct Raw<T> {
    pub parsed: T,
    pub raw: Box<RawValue>,
}

impl<T: DeserializeOwned> Raw<T> {
    /// Parses `T` straight from the bytes captured in `raw` — not from a
    /// re-serialization of an already-parsed value.
    pub fn from_raw_value(raw: Box<RawValue>) -> serde_json::Result<Self> {
        let parsed = serde_json::from_str(raw.get())?;
        Ok(Raw { parsed, raw })
    }
}
