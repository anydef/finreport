//! `ollama`: the native Ollama REST API (`POST {base_url}/api/chat`), no key
//! (§2.9). Distinct wire format from `openai` — `format: "json"` constrains
//! the output rather than an OpenAI-style `response_format`, and the answer
//! comes back as a single `message` object rather than a `choices` array.

use crate::http::{parse_structured_answer, send_with_retry};
use crate::prompt::{system_prompt, user_turn};
use crate::provider::{LabelProvider, LabelRequest, LabelSuggestion, ProviderError};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";
pub const DEFAULT_MODEL: &str = "llama3.1";

pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    timeout: Duration,
}

impl OllamaProvider {
    pub fn new(base_url: String, model: String, timeout: Duration) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            model,
            timeout,
        }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    format: &'static str,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: String,
}

#[async_trait::async_trait]
impl LabelProvider for OllamaProvider {
    fn id(&self) -> &'static str {
        "ollama"
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn suggest(&self, req: &LabelRequest<'_>) -> Result<LabelSuggestion, ProviderError> {
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: system_prompt(req.catalog, req.prompt_version),
                },
                ChatMessage {
                    role: "user",
                    content: user_turn(req),
                },
            ],
            stream: false,
            format: "json",
        };
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));

        let response = send_with_retry(|| self.client.post(&url).json(&body), self.timeout).await?;

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

        let parsed: ChatResponse = serde_json::from_str(&raw)
            .map_err(|err| ProviderError::Parse(format!("{err}: {raw:?}")))?;

        parse_structured_answer(&parsed.message.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::CategoryCatalog;
    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn request<'a>(catalog: &'a CategoryCatalog) -> LabelRequest<'a> {
        LabelRequest {
            counterparty: Some("netflix"),
            description: None,
            amount: Decimal::new(-1499, 2),
            currency: "EUR",
            booking_date: NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(),
            transaction_type: None,
            catalog,
            prompt_version: "2",
        }
    }

    #[tokio::test]
    async fn parses_a_recorded_successful_response() {
        let server = MockServer::start().await;
        let recorded = include_str!("../tests/fixtures/ollama_success.json");
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(recorded, "application/json"))
            .mount(&server)
            .await;

        let provider = OllamaProvider::new(
            server.uri(),
            DEFAULT_MODEL.to_string(),
            Duration::from_secs(5),
        );
        let catalog = CategoryCatalog::default();
        let suggestion = provider.suggest(&request(&catalog)).await.unwrap();

        assert_eq!(
            suggestion.category_slug.as_deref(),
            Some("entertainment.subscriptions")
        );
        assert_eq!(suggestion.confidence, 0.9);
    }

    #[tokio::test]
    async fn maps_server_error_to_provider_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let provider = OllamaProvider::new(
            server.uri(),
            DEFAULT_MODEL.to_string(),
            Duration::from_secs(5),
        );
        let catalog = CategoryCatalog::default();
        let err = provider.suggest(&request(&catalog)).await.unwrap_err();

        assert!(matches!(err, ProviderError::ServerError(_)));
    }

    #[tokio::test]
    async fn garbage_body_maps_to_parse_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let provider = OllamaProvider::new(
            server.uri(),
            DEFAULT_MODEL.to_string(),
            Duration::from_secs(5),
        );
        let catalog = CategoryCatalog::default();
        let err = provider.suggest(&request(&catalog)).await.unwrap_err();

        assert!(matches!(err, ProviderError::Parse(_)));
    }
}
