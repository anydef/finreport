//! Shared HTTP plumbing for the real providers (§2.9): one jittered retry on
//! 429/5xx, a per-request timeout, and parsing of the structured JSON a
//! provider is asked to return. Kept provider-agnostic so `anthropic`,
//! `ollama` and `openai` only own their own request/response shapes.

use crate::provider::{LabelSuggestion, ProviderError};
use rand::Rng;
use serde::Deserialize;
use std::time::Duration;
use tracing::warn;

/// Sends one request, retrying exactly once if the first attempt comes back
/// `429` or `5xx` (§2.9). A timeout never retries — it already waited the
/// full budget once. A transport-level error (connection refused, DNS, ...)
/// is treated like a server error: the caller's instinct to retry once is
/// also useful there, so it gets the same single retry.
pub async fn send_with_retry<F>(
    mut build_request: F,
    timeout: Duration,
) -> Result<reqwest::Response, ProviderError>
where
    F: FnMut() -> reqwest::RequestBuilder,
{
    let mut attempt = 0;
    loop {
        let outcome = tokio::time::timeout(timeout, build_request().send()).await;
        match outcome {
            Err(_elapsed) => return Err(ProviderError::Timeout),
            Ok(Err(err)) => {
                if attempt == 0 {
                    attempt += 1;
                    warn!(error = %err, "provider: transport error, retrying once");
                    jittered_backoff().await;
                    continue;
                }
                return Err(ProviderError::ServerError(err.to_string()));
            }
            Ok(Ok(response)) => {
                let status = response.status();
                let retryable = status.as_u16() == 429 || status.is_server_error();
                if retryable && attempt == 0 {
                    attempt += 1;
                    warn!(%status, "provider: retryable status, retrying once");
                    jittered_backoff().await;
                    continue;
                }
                if status.as_u16() == 429 {
                    return Err(ProviderError::RateLimited);
                }
                if status.is_server_error() {
                    return Err(ProviderError::ServerError(status.to_string()));
                }
                return Ok(response);
            }
        }
    }
}

/// A short, jittered delay between the first attempt and the retry so a
/// pile of concurrent requests do not all retry in lockstep.
async fn jittered_backoff() {
    let millis = rand::rng().random_range(100..=300);
    tokio::time::sleep(Duration::from_millis(millis)).await;
}

/// The structured answer every real provider is instructed to return
/// (§2.9's prompt schema). Deserialized from whatever free-form text the
/// provider wraps it in — see [`extract_json_object`].
#[derive(Debug, Deserialize)]
struct StructuredAnswer {
    category_slug: Option<String>,
    proposed_path: Option<String>,
    confidence: f32,
    #[serde(default)]
    ambiguous: bool,
    reasoning: Option<String>,
}

/// Parses a provider's raw text answer into a [`LabelSuggestion`], clamping
/// confidence to `0.0..=1.0` (§2.9) before it ever reaches the caller. A
/// provider that does not return the expected JSON shape is a
/// [`ProviderError::Parse`], never a label.
pub fn parse_structured_answer(text: &str) -> Result<LabelSuggestion, ProviderError> {
    let json = extract_json_object(text)
        .ok_or_else(|| ProviderError::Parse(format!("no JSON object found in: {text:?}")))?;
    let answer: StructuredAnswer = serde_json::from_str(json)
        .map_err(|err| ProviderError::Parse(format!("{err}: {json:?}")))?;

    Ok(LabelSuggestion {
        category_slug: answer.category_slug,
        proposed_path: answer.proposed_path,
        confidence: answer.confidence.clamp(0.0, 1.0),
        ambiguous: answer.ambiguous,
        reasoning: answer.reasoning,
    })
}

/// Finds the first `{ ... }` span in `text` and returns it verbatim. Models
/// are instructed to answer with exactly one JSON object and nothing else,
/// but some wrap it in a ```json fence anyway; this tolerates that without
/// attempting a full markdown parse.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    Some(&text[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bare_json_object() {
        let text = r#"{"category_slug":"food.groceries","confidence":0.9}"#;
        assert_eq!(extract_json_object(text), Some(text));
    }

    #[test]
    fn extracts_json_object_wrapped_in_markdown_fence() {
        let text = "```json\n{\"category_slug\":\"food.groceries\",\"confidence\":0.9}\n```";
        assert_eq!(
            extract_json_object(text),
            Some(r#"{"category_slug":"food.groceries","confidence":0.9}"#)
        );
    }

    #[test]
    fn no_braces_is_none() {
        assert_eq!(extract_json_object("no json here"), None);
    }

    #[test]
    fn parses_full_structured_answer() {
        let text = r#"{
            "category_slug": "food.groceries",
            "proposed_path": null,
            "confidence": 0.93,
            "ambiguous": false,
            "reasoning": "matches a grocery chain"
        }"#;

        let suggestion = parse_structured_answer(text).unwrap();

        assert_eq!(suggestion.category_slug.as_deref(), Some("food.groceries"));
        assert_eq!(suggestion.proposed_path, None);
        assert_eq!(suggestion.confidence, 0.93);
        assert!(!suggestion.ambiguous);
        assert_eq!(suggestion.reasoning.as_deref(), Some("matches a grocery chain"));
    }

    #[test]
    fn clamps_out_of_range_confidence() {
        let text = r#"{"category_slug":"a","confidence":1.7,"ambiguous":false}"#;
        assert_eq!(parse_structured_answer(text).unwrap().confidence, 1.0);

        let text = r#"{"category_slug":"a","confidence":-0.3,"ambiguous":false}"#;
        assert_eq!(parse_structured_answer(text).unwrap().confidence, 0.0);
    }

    #[test]
    fn missing_confidence_is_a_parse_error_not_a_label() {
        let text = r#"{"category_slug":"a","ambiguous":false}"#;
        assert!(matches!(
            parse_structured_answer(text),
            Err(ProviderError::Parse(_))
        ));
    }

    #[test]
    fn garbage_text_is_a_parse_error() {
        assert!(matches!(
            parse_structured_answer("not json at all"),
            Err(ProviderError::Parse(_))
        ));
    }

    #[test]
    fn ambiguous_defaults_to_false_when_absent() {
        let text = r#"{"category_slug":"a","confidence":0.5}"#;
        assert!(!parse_structured_answer(text).unwrap().ambiguous);
    }

    // --- send_with_retry, against a local mock server (no network) --------

    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn succeeds_on_first_try_with_no_retry() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let response = send_with_retry(
            || client.post(server.uri()),
            Duration::from_millis(500),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn retries_once_on_429_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let response = send_with_retry(
            || client.post(server.uri()),
            Duration::from_millis(500),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn rate_limited_twice_maps_to_provider_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429))
            .expect(2)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let err = send_with_retry(|| client.post(server.uri()), Duration::from_millis(500))
            .await
            .unwrap_err();

        assert_eq!(err, ProviderError::RateLimited);
    }

    #[tokio::test]
    async fn server_error_twice_maps_to_provider_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .expect(2)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let err = send_with_retry(|| client.post(server.uri()), Duration::from_millis(500))
            .await
            .unwrap_err();

        assert!(matches!(err, ProviderError::ServerError(_)));
    }

    #[tokio::test]
    async fn slow_response_times_out_without_retrying() {
        let server = MockServer::start().await;
        // A wide margin between the server's delay and the client's timeout
        // keeps this deterministic even when the whole workspace's test
        // suite is running in parallel and the host is briefly loaded.
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
            .expect(1)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let err = send_with_retry(|| client.post(server.uri()), Duration::from_millis(300))
            .await
            .unwrap_err();

        assert_eq!(err, ProviderError::Timeout);
    }

    #[tokio::test]
    async fn client_error_is_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400))
            .expect(1)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let response = send_with_retry(|| client.post(server.uri()), Duration::from_millis(500))
            .await
            .unwrap();

        assert_eq!(response.status(), 400);
    }
}
