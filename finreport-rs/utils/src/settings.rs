use secrecy::SecretString;
use serde::Deserialize;

/// Sensitive fields are wrapped in `SecretString` from the `secrecy` crate so
/// they never leak via `Debug` / `Display` (printed as `[REDACTED alloc::string::String]`).
/// Call `.expose_secret()` at the boundary where the raw value is required.
#[derive(Deserialize, Debug, Clone)]
pub struct Settings {
    pub client_id: String,
    pub client_secret: SecretString,
    pub zugangsnummer: SecretString,
    pub pin: SecretString,
    pub oauth_url: String,
    pub url: String,
    pub save_file_path: String,
    pub database_url: SecretString,
}


