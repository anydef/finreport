//! `openai`: any OpenAI-compatible `/v1/chat/completions` endpoint at
//! `APP_llm_base_url` — covers llama.cpp, vLLM, LM Studio, Unsloth, or the
//! real OpenAI API (§2.9). The bearer key is optional: most of these local
//! servers do not check it.

use crate::http::{parse_structured_answer, send_with_retry};
use crate::prompt::{system_prompt, user_turn};
use crate::provider::{LabelProvider, LabelRequest, LabelSuggestion, ProviderError};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DEFAULT_MODEL: &str = "gpt-4o-mini";

pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<SecretString>,
    timeout: Duration,
}

impl OpenAiProvider {
    pub fn new(
        base_url: String,
        model: String,
        api_key: Option<SecretString>,
        timeout: Duration,
    ) -> Self {
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
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    response_format: ResponseFormat,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

#[async_trait::async_trait]
impl LabelProvider for OpenAiProvider {
    fn id(&self) -> &'static str {
        "openai"
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
            response_format: ResponseFormat { kind: "json_object" },
        };
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );

        let response = send_with_retry(
            || {
                let builder = self.client.post(&url).json(&body);
                match &self.api_key {
                    Some(key) => builder.bearer_auth(key.expose_secret()),
                    None => builder,
                }
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

        let parsed: ChatResponse = serde_json::from_str(&raw)
            .map_err(|err| ProviderError::Parse(format!("{err}: {raw:?}")))?;
        let text = parsed
            .choices
            .first()
            .map(|choice| choice.message.content.as_str())
            .ok_or_else(|| ProviderError::Parse("no choices in response".to_string()))?;

        parse_structured_answer(text)
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
            counterparty: Some("shell"),
            description: None,
            amount: Decimal::new(-6000, 2),
            currency: "EUR",
            booking_date: NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
            transaction_type: None,
            catalog,
            prompt_version: "2",
        }
    }

    #[tokio::test]
    async fn parses_a_recorded_successful_response() {
        let server = MockServer::start().await;
        let recorded = include_str!("../tests/fixtures/openai_success.json");
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(recorded, "application/json"))
            .mount(&server)
            .await;

        let provider = OpenAiProvider::new(
            server.uri(),
            DEFAULT_MODEL.to_string(),
            None,
            Duration::from_secs(5),
        );
        let catalog = CategoryCatalog::default();
        let suggestion = provider.suggest(&request(&catalog)).await.unwrap();

        assert_eq!(
            suggestion.category_slug.as_deref(),
            Some("transportation.gas")
        );
        assert_eq!(suggestion.confidence, 0.93);
    }

    #[tokio::test]
    async fn maps_429_to_rate_limited_after_the_retry() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let provider = OpenAiProvider::new(
            server.uri(),
            DEFAULT_MODEL.to_string(),
            None,
            Duration::from_secs(5),
        );
        let catalog = CategoryCatalog::default();
        let err = provider.suggest(&request(&catalog)).await.unwrap_err();

        assert_eq!(err, ProviderError::RateLimited);
    }

    #[tokio::test]
    async fn garbage_body_maps_to_parse_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let provider = OpenAiProvider::new(
            server.uri(),
            DEFAULT_MODEL.to_string(),
            None,
            Duration::from_secs(5),
        );
        let catalog = CategoryCatalog::default();
        let err = provider.suggest(&request(&catalog)).await.unwrap_err();

        assert!(matches!(err, ProviderError::Parse(_)));
    }
}
