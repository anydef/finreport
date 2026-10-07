//! `anthropic`: the Messages API (`POST {base_url}/v1/messages`), authenticated
//! with `x-api-key` (§2.9). The only provider that requires a key.

use crate::http::{parse_structured_answer, send_with_retry};
use crate::prompt::{system_prompt, user_turn};
use crate::provider::{LabelProvider, LabelRequest, LabelSuggestion, ProviderError};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-5";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 512;

pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: SecretString,
    timeout: Duration,
}

impl AnthropicProvider {
    pub fn new(base_url: String, model: String, api_key: SecretString, timeout: Duration) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            model,
            api_key,
            timeout,
        }
    }
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: String,
    messages: Vec<Message<'a>>,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: String,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(default)]
    text: String,
}

#[async_trait::async_trait]
impl LabelProvider for AnthropicProvider {
    fn id(&self) -> &'static str {
        "anthropic"
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn suggest(&self, req: &LabelRequest<'_>) -> Result<LabelSuggestion, ProviderError> {
        let body = MessagesRequest {
            model: &self.model,
            max_tokens: MAX_TOKENS,
            system: system_prompt(req.catalog, req.prompt_version),
            messages: vec![Message {
                role: "user",
                content: user_turn(req),
            }],
        };
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));

        let response = send_with_retry(
            || {
                self.client
                    .post(&url)
                    .header("x-api-key", self.api_key.expose_secret())
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .json(&body)
            },
            self.timeout,
        )
        .await?;

        let status = response.status();
        let raw = response
            .text()
            .await
            .map_err(|err| ProviderError::Parse(format!("could not read response body: {err}")))?;
        if !status.is_success() {
            return Err(ProviderError::Parse(format!(
                "unexpected status {status}: {raw}"
            )));
        }

        let parsed: MessagesResponse = serde_json::from_str(&raw)
            .map_err(|err| ProviderError::Parse(format!("{err}: {raw:?}")))?;
        let text = parsed
            .content
            .first()
            .map(|block| block.text.as_str())
            .ok_or_else(|| ProviderError::Parse("no content blocks in response".to_string()))?;

        parse_structured_answer(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::CategoryCatalog;
    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn request<'a>(catalog: &'a CategoryCatalog) -> LabelRequest<'a> {
        LabelRequest {
            counterparty: Some("rewe"),
            description: None,
            amount: Decimal::new(-1250, 2),
            currency: "EUR",
            booking_date: NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            transaction_type: None,
            catalog,
            prompt_version: "2",
        }
    }

    #[tokio::test]
    async fn parses_a_recorded_successful_response() {
        let server = MockServer::start().await;
        let recorded = include_str!("../tests/fixtures/anthropic_success.json");
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(recorded, "application/json"))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::new(
            server.uri(),
            DEFAULT_MODEL.to_string(),
            SecretString::from("test-key".to_string()),
            Duration::from_secs(5),
        );
        let catalog = CategoryCatalog::default();
        let suggestion = provider.suggest(&request(&catalog)).await.unwrap();

        assert_eq!(suggestion.category_slug.as_deref(), Some("food.groceries"));
        assert_eq!(suggestion.confidence, 0.95);
        assert!(!suggestion.ambiguous);
    }

    #[tokio::test]
    async fn maps_429_to_rate_limited_after_the_retry() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::new(
            server.uri(),
            DEFAULT_MODEL.to_string(),
            SecretString::from("test-key".to_string()),
            Duration::from_secs(5),
        );
        let catalog = CategoryCatalog::default();
        let err = provider.suggest(&request(&catalog)).await.unwrap_err();

        assert_eq!(err, ProviderError::RateLimited);
    }

    #[tokio::test]
    async fn maps_server_error_to_provider_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::new(
            server.uri(),
            DEFAULT_MODEL.to_string(),
            SecretString::from("test-key".to_string()),
            Duration::from_secs(5),
        );
        let catalog = CategoryCatalog::default();
        let err = provider.suggest(&request(&catalog)).await.unwrap_err();

        assert!(matches!(err, ProviderError::ServerError(_)));
    }

    #[tokio::test]
    async fn maps_timeout_to_provider_error() {
        let server = MockServer::start().await;
        // Wide margin (see `http::tests::slow_response_times_out_without_retrying`)
        // so this stays deterministic under a loaded, parallel test run.
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::new(
            server.uri(),
            DEFAULT_MODEL.to_string(),
            SecretString::from("test-key".to_string()),
            Duration::from_millis(300),
        );
        let catalog = CategoryCatalog::default();
        let err = provider.suggest(&request(&catalog)).await.unwrap_err();

        assert_eq!(err, ProviderError::Timeout);
    }

    #[tokio::test]
    async fn garbage_body_maps_to_parse_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::new(
            server.uri(),
            DEFAULT_MODEL.to_string(),
            SecretString::from("test-key".to_string()),
            Duration::from_secs(5),
        );
        let catalog = CategoryCatalog::default();
        let err = provider.suggest(&request(&catalog)).await.unwrap_err();

        assert!(matches!(err, ProviderError::Parse(_)));
    }
}
