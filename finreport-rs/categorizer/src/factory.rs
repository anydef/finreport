//! The `APP_llm_provider` factory (§2.9/§4). Validation is lazy here — only
//! called when something actually needs a provider, never at `webapp`/
//! projector startup (§4) — so a missing key only breaks the labeler run
//! that needed it.

use crate::anthropic::{self, AnthropicProvider};
use crate::ollama::{self, OllamaProvider};
use crate::openai::{self, OpenAiProvider};
use crate::provider::fake::FakeProvider;
use crate::provider::{LabelProvider, ProviderError};
use std::time::Duration;
use utils::settings::Settings;

/// Builds the [`LabelProvider`] selected by `settings.llm_provider`
/// (`"fake"` when unset — §4's default). A provider that needs config it
/// does not have returns [`ProviderError::Configuration`] rather than
/// panicking, so the labeler can log and exit cleanly instead of crashing.
pub fn build_provider(settings: &Settings) -> Result<Box<dyn LabelProvider>, ProviderError> {
    let timeout = Duration::from_millis(settings.llm_timeout_ms);

    match settings.llm_provider.as_str() {
        "fake" => Ok(Box::new(FakeProvider::new())),
        "anthropic" => {
            let api_key = settings.anthropic_api_key.clone().ok_or_else(|| {
                ProviderError::Configuration(
                    "APP_anthropic_api_key is required when APP_llm_provider=anthropic"
                        .to_string(),
                )
            })?;
            let base_url = settings
                .llm_base_url
                .clone()
                .unwrap_or_else(|| anthropic::DEFAULT_BASE_URL.to_string());
            let model = settings
                .llm_model
                .clone()
                .unwrap_or_else(|| anthropic::DEFAULT_MODEL.to_string());
            Ok(Box::new(AnthropicProvider::new(
                base_url, model, api_key, timeout,
            )))
        }
        "ollama" => {
            let base_url = settings
                .llm_base_url
                .clone()
                .unwrap_or_else(|| ollama::DEFAULT_BASE_URL.to_string());
            let model = settings
                .llm_model
                .clone()
                .unwrap_or_else(|| ollama::DEFAULT_MODEL.to_string());
            Ok(Box::new(OllamaProvider::new(base_url, model, timeout)))
        }
        "openai" => {
            let base_url = settings.llm_base_url.clone().ok_or_else(|| {
                ProviderError::Configuration(
                    "APP_llm_base_url is required when APP_llm_provider=openai".to_string(),
                )
            })?;
            let model = settings
                .llm_model
                .clone()
                .unwrap_or_else(|| openai::DEFAULT_MODEL.to_string());
            Ok(Box::new(OpenAiProvider::new(
                base_url,
                model,
                settings.llm_api_key.clone(),
                timeout,
            )))
        }
        other => Err(ProviderError::Configuration(format!(
            "unknown APP_llm_provider {other:?}; expected fake|anthropic|ollama|openai"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::SecretString;

    /// A `Settings` with every field at its documented §4 default — built by
    /// struct literal rather than `Settings::from_env`/`from_source` (both
    /// private to `utils`), so these tests do not depend on process env vars
    /// at all.
    fn base_settings() -> Settings {
        Settings {
            oauth_url: None,
            url: None,
            save_file_path: None,
            database_url: None,
            kafka_brokers: None,
            cookie_secure: true,
            allowed_origins: String::new(),
            session_ttl_days: 30,
            projector_default_owner: None,
        admin_username: "admin".to_string(),
        admin_password: None,
            llm_provider: "fake".to_string(),
            anthropic_api_key: None,
            llm_api_key: None,
            llm_base_url: None,
            llm_model: None,
            llm_timeout_ms: 20_000,
            llm_min_confidence: 0.5,
            llm_max_requests_per_run: 200,
            prompt_version: "2".to_string(),
            rule_learn_min_observations: 3,
            rule_auto_approve_threshold: 0.9,
            labeler_max_projection_lag: 0,
            transfer_match_days: 3,
            recurring_min_occurrences: 3,
            recurring_amount_tolerance: 0.10,
            recurring_window_months: 18,
            max_tags_per_transaction: 10,
            accounts: Default::default(),
            account_name: None,
            client_id: None,
            client_secret: None,
            zugangsnummer: None,
            pin: None,
        }
    }

    #[test]
    fn defaults_to_fake_when_unset() {
        let settings = base_settings();
        assert_eq!(settings.llm_provider, "fake");

        let provider = build_provider(&settings).unwrap();
        assert_eq!(provider.id(), "fake");
    }

    #[test]
    fn anthropic_without_key_is_a_configuration_error() {
        let mut settings = base_settings();
        settings.llm_provider = "anthropic".to_string();

        let err = match build_provider(&settings) {
            Err(err) => err,
            Ok(_) => panic!("expected a configuration error"),
        };
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[test]
    fn anthropic_with_key_builds_with_defaults() {
        let mut settings = base_settings();
        settings.llm_provider = "anthropic".to_string();
        settings.anthropic_api_key = Some(SecretString::from("sk-test".to_string()));

        let provider = build_provider(&settings).unwrap();
        assert_eq!(provider.id(), "anthropic");
        assert_eq!(provider.model(), anthropic::DEFAULT_MODEL);
    }

    #[test]
    fn ollama_needs_no_key() {
        let mut settings = base_settings();
        settings.llm_provider = "ollama".to_string();

        let provider = build_provider(&settings).unwrap();
        assert_eq!(provider.id(), "ollama");
        assert_eq!(provider.model(), ollama::DEFAULT_MODEL);
    }

    #[test]
    fn openai_without_base_url_is_a_configuration_error() {
        let mut settings = base_settings();
        settings.llm_provider = "openai".to_string();

        let err = match build_provider(&settings) {
            Err(err) => err,
            Ok(_) => panic!("expected a configuration error"),
        };
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[test]
    fn openai_with_base_url_builds_without_a_key() {
        let mut settings = base_settings();
        settings.llm_provider = "openai".to_string();
        settings.llm_base_url = Some("http://localhost:8000".to_string());

        let provider = build_provider(&settings).unwrap();
        assert_eq!(provider.id(), "openai");
    }

    #[test]
    fn unknown_provider_is_a_configuration_error() {
        let mut settings = base_settings();
        settings.llm_provider = "bogus".to_string();

        let err = match build_provider(&settings) {
            Err(err) => err,
            Ok(_) => panic!("expected a configuration error"),
        };
        assert!(matches!(err, ProviderError::Configuration(_)));
    }
}
