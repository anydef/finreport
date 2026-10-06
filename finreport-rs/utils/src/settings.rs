use config::{Config, ConfigError, Environment};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

/// A single Comdirect login as configured under `APP_accounts__<key>__*`
/// (e.g. `APP_accounts__0__zugangsnummer`).
///
/// Sensitive fields are wrapped in `SecretString` from the `secrecy` crate so
/// they never leak via `Debug` / `Display` (printed as `[REDACTED alloc::string::String]`).
/// Call `.expose_secret()` at the boundary where the raw value is required.
#[derive(Deserialize, Debug, Clone)]
pub struct ComdirectAccount {
    /// Human-readable label for this login, stored on every account row it
    /// imports (`account.account_name`). Display only — accounts are always
    /// identified by their key, never by this.
    pub name: Option<String>,
    pub client_id: String,
    pub client_secret: SecretString,
    pub zugangsnummer: SecretString,
    pub pin: SecretString,
    /// Where this login's session tokens are persisted. Defaults to the global
    /// `save_file_path` with the account key spliced in (`.session.0.json`).
    pub save_file_path: Option<String>,
}

/// Everything one Comdirect login needs: its credentials, the shared API URLs
/// and the session file that belongs to it alone.
///
/// The importer runs one task per profile, so each login approves its own
/// push-TAN and refreshes its own session independently — see
/// `webapp/src/bin/import_transactions.rs`.
#[derive(Debug, Clone)]
pub struct ComdirectProfile {
    /// Key this profile was configured under; `"default"` for the flat
    /// single-account form. This is the account's identifier: `--account`
    /// selects on it and the session file name is derived from it.
    pub key: String,
    /// Label persisted to `account.account_name` for everything this login
    /// imports, so accounts can be told apart by whose login they came from.
    /// `None` when unset: it is a display label, so it has no default to fall
    /// back to — nothing resolves an account through it.
    pub name: Option<String>,
    pub client_id: String,
    pub client_secret: SecretString,
    pub zugangsnummer: SecretString,
    pub pin: SecretString,
    pub oauth_url: String,
    pub url: String,
    pub save_file_path: String,
}

/// Key used for the legacy flat `APP_client_id` / `APP_pin` / ... form.
pub const DEFAULT_ACCOUNT_KEY: &str = "default";

#[derive(Deserialize, Debug, Clone)]
pub struct Settings {
    /// Comdirect OAuth base URL. Only the importer entry points
    /// (`import-transactions`, `init-session`) need this — `webapp` and the
    /// projector never call the Comdirect API — so it is `None` rather than
    /// required, and validated lazily via [`Settings::profiles`] /
    /// [`Settings::select_profile`] the moment a profile is actually built.
    pub oauth_url: Option<String>,
    /// Comdirect REST API base URL; same optionality as `oauth_url`.
    pub url: Option<String>,
    /// Base path for a Comdirect login's session-token file; same
    /// optionality as `oauth_url`.
    pub save_file_path: Option<String>,
    /// Postgres connection string. Only binaries that touch Postgres
    /// (`webapp`, `user-admin`, `legacy-backfill`) need this — the importer
    /// writes only to Kafka (§2.6) — so it is `None` rather than required;
    /// use [`Settings::require_database_url`] at the entry points that do
    /// need it.
    pub database_url: Option<SecretString>,

    /// Kafka bootstrap servers for the event-log dual-write, e.g.
    /// `kafka.lab.anydef.de:9092` — central homelab infrastructure, not
    /// deployed by this repo. Unset disables publishing entirely: during the
    /// migration Postgres is still the source of truth, so an importer with no
    /// broker configured is a supported setup (local dev runs this way).
    /// `webapp` (the GraphQL server) keeps treating this as optional; the
    /// importer entry point additionally calls `require_kafka_brokers`,
    /// which turns a missing value into a startup error (§2.6).
    pub kafka_brokers: Option<String>,

    /// `Secure` flag on the `fr_session` cookie (§4). Defaults to `true`;
    /// local dev's `.env` sets this `false` so plain-HTTP localhost still
    /// gets the cookie back.
    #[serde(default = "default_cookie_secure")]
    pub cookie_secure: bool,

    /// Comma-separated CORS allow-list checked against the request's `Origin`
    /// header (§4). Stored as the raw string (rather than `Vec<String>`,
    /// which config-rs's env source does not parse out of a scalar value) —
    /// use `allowed_origins()` to get the parsed, trimmed list.
    #[serde(default)]
    pub allowed_origins: String,

    /// Session TTL in days (§4). `user_session.expires_at` is set this far
    /// out from login; the sliding refresh only ever extends it.
    #[serde(default = "default_session_ttl_days")]
    pub session_ttl_days: i64,

    /// Username the projector links every newly projected account to, in the
    /// same transaction (§4). Unset: accounts stay unlinked until an explicit
    /// `user-admin link`.
    pub projector_default_owner: Option<String>,

    /// Comdirect logins keyed by the segment in `APP_accounts__<key>__*`.
    /// A `BTreeMap` rather than a `Vec` because config-rs turns numbered env
    /// segments into a table keyed by `"0"`, `"1"`, ... — sorting by key keeps
    /// the numbering meaningful.
    ///
    /// The key is the stable identifier (it selects the account and names its
    /// session file), so it wants to stay put; `__name` carries the
    /// human-readable label and is free to change.
    #[serde(default)]
    pub accounts: BTreeMap<String, ComdirectAccount>,

    // Single-account form, kept so existing `.env` files and the deployed
    // stack keep working. Ignored when `accounts` is non-empty.
    pub account_name: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<SecretString>,
    pub zugangsnummer: Option<SecretString>,
    pub pin: Option<SecretString>,
}

#[derive(Debug)]
pub enum SettingsError {
    /// Neither `APP_accounts__*` nor the flat `APP_client_id` / ... were set.
    NoAccountsConfigured,
    /// An account was configured, but some of its credentials are blank.
    IncompleteAccount { key: String, missing: Vec<String> },
    /// `--account <key>` named a profile that isn't configured.
    UnknownAccount {
        requested: String,
        available: Vec<String>,
    },
    /// More than one profile is configured and none was selected.
    AmbiguousAccount { available: Vec<String> },
    /// The importer entry point requires `APP_kafka_brokers`; the GraphQL
    /// server does not (§2.6).
    MissingKafkaBrokers,
    /// A binary that touches Postgres (`webapp`, `user-admin`,
    /// `legacy-backfill`) requires `APP_database_url`; the importer does not
    /// (it writes only to Kafka).
    MissingDatabaseUrl,
    /// Building a Comdirect profile requires `oauth_url`/`url`/
    /// `save_file_path`, shared across every configured login; `webapp` and
    /// the projector never build a profile and so never hit this.
    MissingComdirectUrls { missing: Vec<String> },
}

impl Display for SettingsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::NoAccountsConfigured => write!(
                f,
                "no Comdirect account configured; set APP_accounts__0__client_id / \
                 __client_secret / __zugangsnummer / __pin (or the flat APP_client_id / \
                 APP_client_secret / APP_zugangsnummer / APP_pin)"
            ),
            SettingsError::IncompleteAccount { key, missing } => write!(
                f,
                "account {:?} is missing credentials: {}",
                key,
                missing.join(", ")
            ),
            SettingsError::UnknownAccount {
                requested,
                available,
            } => write!(
                f,
                "unknown account {:?}; configured accounts: {}",
                requested,
                available.join(", ")
            ),
            SettingsError::AmbiguousAccount { available } => write!(
                f,
                "{} accounts are configured ({}) — pick one by key with \
                 --account <key>",
                available.len(),
                available.join(", ")
            ),
            SettingsError::MissingKafkaBrokers => write!(
                f,
                "APP_kafka_brokers is required for the importer; the event log \
                 has no broker configured"
            ),
            SettingsError::MissingDatabaseUrl => write!(
                f,
                "APP_database_url is required to connect to Postgres"
            ),
            SettingsError::MissingComdirectUrls { missing } => write!(
                f,
                "Comdirect configuration is missing: {}",
                missing.join(", ")
            ),
        }
    }
}

impl std::error::Error for SettingsError {}

impl Settings {
    /// Load settings from `APP_*` environment variables. `__` separates nested
    /// keys, so `APP_accounts__0__pin` lands in `accounts["0"].pin`.
    pub fn from_env() -> Result<Settings, ConfigError> {
        Self::from_source(env_source())
    }

    /// `APP_kafka_brokers`, parsed and trimmed, or an error. The importer
    /// entry point calls this instead of reading `kafka_brokers` directly, so
    /// it fails fast on startup rather than silently running with publishing
    /// disabled (§2.6); `webapp` keeps using the field as optional.
    pub fn require_kafka_brokers(&self) -> Result<&str, SettingsError> {
        self.kafka_brokers
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(SettingsError::MissingKafkaBrokers)
    }

    /// `APP_database_url`, or an error — the entry points that touch
    /// Postgres (`webapp`, `user-admin`, `legacy-backfill`) call this
    /// instead of reading `database_url` directly, so a missing value fails
    /// fast on startup rather than surfacing as an `Option::unwrap` panic.
    pub fn require_database_url(&self) -> Result<&SecretString, SettingsError> {
        self.database_url
            .as_ref()
            .ok_or(SettingsError::MissingDatabaseUrl)
    }

    /// `oauth_url`/`url`/`save_file_path`, trimmed and validated non-blank,
    /// or the names of whichever are missing. Shared by `profile_from` and
    /// `flat_profile` — every profile needs the same three values.
    fn require_comdirect_urls(&self) -> Result<(&str, &str, &str), SettingsError> {
        fn non_blank(value: &Option<String>) -> Option<&str> {
            value.as_deref().map(str::trim).filter(|v| !v.is_empty())
        }

        let oauth_url = non_blank(&self.oauth_url);
        let url = non_blank(&self.url);
        let save_file_path = non_blank(&self.save_file_path);

        let missing: Vec<String> = [
            ("oauth_url", oauth_url),
            ("url", url),
            ("save_file_path", save_file_path),
        ]
        .into_iter()
        .filter(|(_, value)| value.is_none())
        .map(|(name, _)| name.to_string())
        .collect();

        if !missing.is_empty() {
            return Err(SettingsError::MissingComdirectUrls { missing });
        }

        Ok((oauth_url.unwrap(), url.unwrap(), save_file_path.unwrap()))
    }

    /// `APP_allowed_origins`, split on commas and trimmed, blank entries
    /// dropped. Empty when unset — the caller treats an empty list as "allow
    /// nothing" (fail closed), not "allow all".
    pub fn allowed_origins(&self) -> Vec<String> {
        self.allowed_origins
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Every configured Comdirect login, ordered by account key.
    pub fn profiles(&self) -> Result<Vec<ComdirectProfile>, SettingsError> {
        if self.accounts.is_empty() {
            return Ok(vec![self.flat_profile()?]);
        }

        self.accounts
            .iter()
            .map(|(key, credentials)| self.profile_from(key, credentials))
            .collect()
    }

    /// Resolve the login an importer process should drive. `selector` is the
    /// account key — never the `__name` label — and may be omitted only when a
    /// single login is configured.
    pub fn select_profile(
        &self,
        selector: Option<&str>,
    ) -> Result<ComdirectProfile, SettingsError> {
        let mut profiles = self.profiles()?;

        match selector {
            Some(key) => profiles
                .into_iter()
                .find(|profile| profile.key == key)
                .ok_or_else(|| SettingsError::UnknownAccount {
                    requested: key.to_string(),
                    available: self.account_keys(),
                }),
            None if profiles.len() == 1 => Ok(profiles.remove(0)),
            None => Err(SettingsError::AmbiguousAccount {
                available: self.account_keys(),
            }),
        }
    }

    fn from_source(source: Environment) -> Result<Settings, ConfigError> {
        Config::builder()
            .add_source(source)
            .build()?
            .try_deserialize::<Settings>()
    }

    fn account_keys(&self) -> Vec<String> {
        if self.accounts.is_empty() {
            vec![DEFAULT_ACCOUNT_KEY.to_string()]
        } else {
            self.accounts.keys().cloned().collect()
        }
    }

    fn profile_from(
        &self,
        key: &str,
        credentials: &ComdirectAccount,
    ) -> Result<ComdirectProfile, SettingsError> {
        let missing = blank_fields(credentials);
        if !missing.is_empty() {
            return Err(SettingsError::IncompleteAccount {
                key: key.to_string(),
                missing,
            });
        }

        let (oauth_url, url, save_file_path) = self.require_comdirect_urls()?;

        Ok(ComdirectProfile {
            key: key.to_string(),
            name: display_name(&credentials.name),
            client_id: credentials.client_id.clone(),
            client_secret: credentials.client_secret.clone(),
            zugangsnummer: credentials.zugangsnummer.clone(),
            pin: credentials.pin.clone(),
            oauth_url: oauth_url.to_string(),
            url: url.to_string(),
            save_file_path: credentials
                .save_file_path
                .clone()
                .unwrap_or_else(|| session_path_for(save_file_path, key)),
        })
    }

    /// The flat `APP_client_id` / ... form as a single profile. Keeps its
    /// session file at the unsuffixed `save_file_path` so an already-deployed
    /// importer doesn't lose its tokens on upgrade.
    fn flat_profile(&self) -> Result<ComdirectProfile, SettingsError> {
        let credentials = ComdirectAccount {
            name: self.account_name.clone(),
            client_id: self.client_id.clone().unwrap_or_default(),
            client_secret: secret_or_empty(&self.client_secret),
            zugangsnummer: secret_or_empty(&self.zugangsnummer),
            pin: secret_or_empty(&self.pin),
            save_file_path: self.save_file_path.clone(),
        };

        // Docker compose passes these as `${APP_client_id:-}`, so "unset" shows
        // up as an empty string just as often as as a missing variable.
        if blank_fields(&credentials).len() == 4 {
            return Err(SettingsError::NoAccountsConfigured);
        }

        self.profile_from(DEFAULT_ACCOUNT_KEY, &credentials)
    }
}

/// Normalises a configured label: whitespace-only counts as unset. Never falls
/// back to the account key — the key identifies the account, the label
/// describes it, and standing in for one another would blur the two.
fn display_name(name: &Option<String>) -> Option<String> {
    name.as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn secret_or_empty(secret: &Option<SecretString>) -> SecretString {
    secret
        .clone()
        .unwrap_or_else(|| SecretString::from(String::new()))
}

/// Names of the credential fields that are absent or blank.
fn blank_fields(credentials: &ComdirectAccount) -> Vec<String> {
    [
        ("client_id", credentials.client_id.as_str()),
        ("client_secret", credentials.client_secret.expose_secret()),
        ("zugangsnummer", credentials.zugangsnummer.expose_secret()),
        ("pin", credentials.pin.expose_secret()),
    ]
    .into_iter()
    .filter(|(_, value)| value.trim().is_empty())
    .map(|(name, _)| name.to_string())
    .collect()
}

/// `APP_cookie_secure` default: secure-by-default, opted out explicitly for
/// plain-HTTP local dev (§4).
fn default_cookie_secure() -> bool {
    true
}

/// `APP_session_ttl_days` default (§4).
fn default_session_ttl_days() -> i64 {
    30
}

fn env_source() -> Environment {
    Environment::with_prefix("APP")
        .prefix_separator("_")
        .separator("__")
}

/// `.session.json` + key `0` → `.session.0.json`, so every login persists its
/// tokens separately even when they share a data volume.
fn session_path_for(base: &str, key: &str) -> String {
    match base.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !stem.ends_with('/') => {
            format!("{stem}.{key}.{extension}")
        }
        _ => format!("{base}.{key}"),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use secrecy::ExposeSecret;

    /// Build settings from an explicit env map instead of the process
    /// environment, so tests stay independent of each other.
    fn settings_from(vars: &[(&str, &str)]) -> Settings {
        let source = env_source().source(Some(
            vars.iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        ));
        Settings::from_source(source).expect("could not load settings")
    }

    fn base_vars() -> Vec<(&'static str, &'static str)> {
        vec![
            ("APP_database_url", "postgresql://localhost/finreport"),
            ("APP_oauth_url", "https://api.comdirect.de"),
            ("APP_url", "https://api.comdirect.de/api"),
            ("APP_save_file_path", ".session.json"),
        ]
    }

    #[test]
    fn numbered_env_vars_become_separate_profiles() {
        let mut vars = base_vars();
        vars.extend([
            ("APP_accounts__0__client_id", "id-0"),
            ("APP_accounts__0__client_secret", "secret-0"),
            ("APP_accounts__0__zugangsnummer", "zugang-0"),
            ("APP_accounts__0__pin", "pin-0"),
            ("APP_accounts__1__client_id", "id-1"),
            ("APP_accounts__1__client_secret", "secret-1"),
            ("APP_accounts__1__zugangsnummer", "zugang-1"),
            ("APP_accounts__1__pin", "pin-1"),
        ]);

        let profiles = settings_from(&vars).profiles().unwrap();

        assert_eq!(
            profiles.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(),
            ["0", "1"]
        );
        assert_eq!(profiles[1].client_id, "id-1");
        assert_eq!(profiles[1].zugangsnummer.expose_secret(), "zugang-1");
        // Shared config is copied onto every profile.
        assert_eq!(profiles[0].url, "https://api.comdirect.de/api");
    }

    #[test]
    fn each_profile_gets_its_own_session_file() {
        let mut vars = base_vars();
        vars.extend([
            ("APP_accounts__0__client_id", "id-0"),
            ("APP_accounts__0__client_secret", "secret-0"),
            ("APP_accounts__0__zugangsnummer", "zugang-0"),
            ("APP_accounts__0__pin", "pin-0"),
            ("APP_accounts__1__client_id", "id-1"),
            ("APP_accounts__1__client_secret", "secret-1"),
            ("APP_accounts__1__zugangsnummer", "zugang-1"),
            ("APP_accounts__1__pin", "pin-1"),
            ("APP_accounts__1__save_file_path", "/data/joint.json"),
        ]);

        let profiles = settings_from(&vars).profiles().unwrap();

        assert_eq!(profiles[0].save_file_path, ".session.0.json");
        // An explicit per-account path wins over the derived one.
        assert_eq!(profiles[1].save_file_path, "/data/joint.json");
    }

    #[test]
    fn flat_credentials_still_load_as_a_single_profile() {
        let mut vars = base_vars();
        vars.extend([
            ("APP_client_id", "id"),
            ("APP_client_secret", "secret"),
            ("APP_zugangsnummer", "zugang"),
            ("APP_pin", "pin"),
        ]);

        let profiles = settings_from(&vars).profiles().unwrap();

        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].key, DEFAULT_ACCOUNT_KEY);
        // Unsuffixed, so an already-deployed importer keeps its tokens.
        assert_eq!(profiles[0].save_file_path, ".session.json");
    }

    #[test]
    fn numbered_accounts_take_precedence_over_flat_credentials() {
        let mut vars = base_vars();
        vars.extend([
            ("APP_client_id", "flat-id"),
            ("APP_client_secret", "flat-secret"),
            ("APP_zugangsnummer", "flat-zugang"),
            ("APP_pin", "flat-pin"),
            ("APP_accounts__0__client_id", "id-0"),
            ("APP_accounts__0__client_secret", "secret-0"),
            ("APP_accounts__0__zugangsnummer", "zugang-0"),
            ("APP_accounts__0__pin", "pin-0"),
        ]);

        let profiles = settings_from(&vars).profiles().unwrap();

        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].client_id, "id-0");
    }

    #[test]
    fn selecting_without_a_key_works_only_for_a_single_account() {
        let mut vars = base_vars();
        vars.extend([
            ("APP_accounts__0__client_id", "id-0"),
            ("APP_accounts__0__client_secret", "secret-0"),
            ("APP_accounts__0__zugangsnummer", "zugang-0"),
            ("APP_accounts__0__pin", "pin-0"),
        ]);
        let single = settings_from(&vars);
        assert_eq!(single.select_profile(None).unwrap().key, "0");

        vars.extend([
            ("APP_accounts__1__client_id", "id-1"),
            ("APP_accounts__1__client_secret", "secret-1"),
            ("APP_accounts__1__zugangsnummer", "zugang-1"),
            ("APP_accounts__1__pin", "pin-1"),
        ]);
        let multiple = settings_from(&vars);
        assert!(matches!(
            multiple.select_profile(None),
            Err(SettingsError::AmbiguousAccount { .. })
        ));
        assert_eq!(multiple.select_profile(Some("1")).unwrap().client_id, "id-1");
        assert!(matches!(
            multiple.select_profile(Some("2")),
            Err(SettingsError::UnknownAccount { .. })
        ));
    }

    #[test]
    fn missing_credentials_are_reported_rather_than_panicking() {
        let settings = settings_from(&base_vars());
        assert!(matches!(
            settings.profiles(),
            Err(SettingsError::NoAccountsConfigured)
        ));
    }

    #[test]
    fn account_names_are_labels_only_and_never_stand_in_for_the_key() {
        let mut vars = base_vars();
        vars.extend([
            ("APP_accounts__0__name", "Pavlo"),
            ("APP_accounts__0__client_id", "id-0"),
            ("APP_accounts__0__client_secret", "secret-0"),
            ("APP_accounts__0__zugangsnummer", "zugang-0"),
            ("APP_accounts__0__pin", "pin-0"),
            ("APP_accounts__1__name", "   "),
            ("APP_accounts__1__client_id", "id-1"),
            ("APP_accounts__1__client_secret", "secret-1"),
            ("APP_accounts__1__zugangsnummer", "zugang-1"),
            ("APP_accounts__1__pin", "pin-1"),
        ]);

        let profiles = settings_from(&vars).profiles().unwrap();

        assert_eq!(profiles[0].name.as_deref(), Some("Pavlo"));
        // A whitespace-only label counts as unset, and an unset label stays
        // unset: it must never borrow the key, which is the identifier.
        assert_eq!(profiles[1].name, None);
        assert_eq!(profiles[1].key, "1");
    }

    #[test]
    fn accounts_are_selected_by_key_not_by_name() {
        let mut vars = base_vars();
        vars.extend([
            ("APP_accounts__0__name", "Joint"),
            ("APP_accounts__0__client_id", "id-0"),
            ("APP_accounts__0__client_secret", "secret-0"),
            ("APP_accounts__0__zugangsnummer", "zugang-0"),
            ("APP_accounts__0__pin", "pin-0"),
            ("APP_accounts__1__client_id", "id-1"),
            ("APP_accounts__1__client_secret", "secret-1"),
            ("APP_accounts__1__zugangsnummer", "zugang-1"),
            ("APP_accounts__1__pin", "pin-1"),
        ]);
        let settings = settings_from(&vars);

        assert_eq!(settings.select_profile(Some("0")).unwrap().client_id, "id-0");
        // The label is not an address: selecting by it finds nothing.
        assert!(matches!(
            settings.select_profile(Some("Joint")),
            Err(SettingsError::UnknownAccount { .. })
        ));
    }

    #[test]
    fn the_flat_form_can_be_named_too() {
        let mut vars = base_vars();
        vars.extend([
            ("APP_client_id", "id"),
            ("APP_client_secret", "secret"),
            ("APP_zugangsnummer", "zugang"),
            ("APP_pin", "pin"),
        ]);
        let unnamed = settings_from(&vars).profiles().unwrap();
        assert_eq!(unnamed[0].key, "default");
        assert_eq!(unnamed[0].name, None);

        vars.push(("APP_account_name", "Joint"));
        assert_eq!(
            settings_from(&vars).profiles().unwrap()[0].name.as_deref(),
            Some("Joint")
        );
    }

    #[test]
    fn blank_flat_credentials_read_as_nothing_configured() {
        // What `${APP_client_id:-}` in docker-compose expands to when the
        // variable isn't set on the stack.
        let mut vars = base_vars();
        vars.extend([
            ("APP_client_id", ""),
            ("APP_client_secret", ""),
            ("APP_zugangsnummer", ""),
            ("APP_pin", ""),
        ]);

        assert!(matches!(
            settings_from(&vars).profiles(),
            Err(SettingsError::NoAccountsConfigured)
        ));
    }

    #[test]
    fn partially_blank_credentials_name_what_is_missing() {
        let mut vars = base_vars();
        vars.extend([
            ("APP_accounts__0__client_id", "id-0"),
            ("APP_accounts__0__client_secret", "secret-0"),
            ("APP_accounts__0__zugangsnummer", ""),
            ("APP_accounts__0__pin", "   "),
        ]);

        match settings_from(&vars).profiles() {
            Err(SettingsError::IncompleteAccount { key, missing }) => {
                assert_eq!(key, "0");
                assert_eq!(missing, ["zugangsnummer", "pin"]);
            }
            other => panic!("expected IncompleteAccount, got {other:?}"),
        }
    }

    #[test]
    fn session_paths_are_derived_from_the_base_path() {
        assert_eq!(session_path_for(".session.json", "0"), ".session.0.json");
        assert_eq!(
            session_path_for("/app/data/.session.json", "joint"),
            "/app/data/.session.joint.json"
        );
        assert_eq!(session_path_for("session", "0"), "session.0");
    }

    /// `webapp`/`user-admin`/`legacy-backfill` load settings with no
    /// Comdirect configuration at all — not even `oauth_url`/`url`/
    /// `save_file_path` — and `import-transactions`/`init-session` load with
    /// no `database_url`. Neither is a deserialization error; each is only
    /// validated by the accessor the binary that actually needs it calls.
    #[test]
    fn database_url_and_comdirect_urls_are_independently_optional() {
        // No database_url at all: fine for a binary that never touches it,
        // caught only by `require_database_url`.
        let webapp_like = settings_from(&[
            ("APP_accounts__0__client_id", "id-0"),
            ("APP_accounts__0__client_secret", "secret-0"),
            ("APP_accounts__0__zugangsnummer", "zugang-0"),
            ("APP_accounts__0__pin", "pin-0"),
        ]);
        assert!(webapp_like.database_url.is_none());
        assert!(matches!(
            webapp_like.require_database_url(),
            Err(SettingsError::MissingDatabaseUrl)
        ));

        // No oauth_url/url/save_file_path at all: fine for a binary that
        // never builds a Comdirect profile; `profiles()` reports exactly
        // what's missing only once something actually asks for a profile.
        let importer_like = settings_from(&[("APP_database_url", "postgresql://localhost/finreport")]);
        assert!(importer_like.oauth_url.is_none());
        assert!(importer_like.url.is_none());
        assert!(importer_like.save_file_path.is_none());
        assert!(
            importer_like
                .require_database_url()
                .unwrap()
                .expose_secret()
                == "postgresql://localhost/finreport"
        );

        let mut vars: Vec<(&str, &str)> = vec![("APP_database_url", "postgresql://localhost/finreport")];
        vars.extend([
            ("APP_accounts__0__client_id", "id-0"),
            ("APP_accounts__0__client_secret", "secret-0"),
            ("APP_accounts__0__zugangsnummer", "zugang-0"),
            ("APP_accounts__0__pin", "pin-0"),
        ]);
        match settings_from(&vars).profiles() {
            Err(SettingsError::MissingComdirectUrls { missing }) => {
                assert_eq!(missing, ["oauth_url", "url", "save_file_path"]);
            }
            other => panic!("expected MissingComdirectUrls, got {other:?}"),
        }
    }

    #[test]
    fn new_wp0_settings_default_when_unset() {
        let settings = settings_from(&base_vars());

        assert!(settings.cookie_secure);
        assert_eq!(settings.session_ttl_days, 30);
        assert_eq!(settings.projector_default_owner, None);
        assert_eq!(settings.allowed_origins(), Vec::<String>::new());
        assert!(matches!(
            settings.require_kafka_brokers(),
            Err(SettingsError::MissingKafkaBrokers)
        ));
    }

    #[test]
    fn allowed_origins_is_parsed_from_a_comma_separated_list() {
        let mut vars = base_vars();
        vars.push((
            "APP_allowed_origins",
            " https://app.example.com ,https://admin.example.com,",
        ));

        assert_eq!(
            settings_from(&vars).allowed_origins(),
            vec!["https://app.example.com", "https://admin.example.com"]
        );
    }

    #[test]
    fn kafka_brokers_can_be_required_or_left_optional() {
        let mut vars = base_vars();
        vars.push(("APP_kafka_brokers", "kafka.lab.anydef.de:9092"));

        let settings = settings_from(&vars);
        assert_eq!(
            settings.require_kafka_brokers().unwrap(),
            "kafka.lab.anydef.de:9092"
        );
        // webapp keeps reading the field directly, unaffected by the
        // importer-only accessor.
        assert_eq!(
            settings.kafka_brokers.as_deref(),
            Some("kafka.lab.anydef.de:9092")
        );
    }

    #[test]
    fn cookie_secure_can_be_disabled_for_local_dev() {
        let mut vars = base_vars();
        vars.push(("APP_cookie_secure", "false"));

        assert!(!settings_from(&vars).cookie_secure);
    }
}
