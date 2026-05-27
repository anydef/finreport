use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use std::time::Duration;

/// Build a reqwest client wrapped with retry middleware that transparently
/// handles transient HTTP errors (including 429 Too Many Requests) with
/// exponential backoff. All Comdirect calls should go through this.
pub fn build_client() -> ClientWithMiddleware {
    let raw = reqwest::Client::builder()
        .connection_verbose(false)
        .build()
        .expect("failed to build reqwest client");
    let retry_policy = ExponentialBackoff::builder()
        .retry_bounds(Duration::from_mins(1), Duration::from_mins(9))
        .build_with_max_retries(6);
    ClientBuilder::new(raw)
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build()
}
