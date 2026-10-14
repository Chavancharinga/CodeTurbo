//! Shared HTTP plumbing: POST with bounded retries, then stream SSE frames.

use crate::{ProviderError, sse::SseParser};
use futures_util::StreamExt;
use std::time::Duration;

const MAX_RETRIES: u32 = 3;

fn retryable(status: u16) -> bool {
    status == 408 || status == 409 || status == 429 || status >= 500
}

/// Rate-limit headers of a response, sorted by name.
fn rate_limits(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = headers
        .iter()
        .filter(|(k, _)| k.as_str().contains("ratelimit"))
        .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
        .collect();
    v.sort();
    v
}

/// What the stream hands its caller.
pub(crate) enum Frame<'a> {
    /// One SSE data payload.
    Data(&'a str),
    /// The request is being retried; a message for the user.
    Retry(String),
}

/// An HTTP client that gives up on a dead connection instead of waiting
/// forever: 15s to connect, 180s without any byte from the server.
pub(crate) fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(180))
        .build()
        .unwrap_or_default()
}

/// Sends the request (retrying 408/409/429/5xx with backoff, honoring
/// `retry-after`) and calls `on_frame` with each SSE data payload and each
/// retry. Returns the response's rate-limit headers.
pub(crate) async fn stream_sse(
    build: impl Fn() -> reqwest::RequestBuilder,
    mut on_frame: impl FnMut(Frame<'_>) -> Result<bool, ProviderError>,
) -> Result<Vec<(String, String)>, ProviderError> {
    let mut attempt = 0;
    let resp = loop {
        let resp = build().send().await?;
        let status = resp.status().as_u16();
        if resp.status().is_success() {
            break resp;
        }
        if attempt < MAX_RETRIES && retryable(status) {
            let wait = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok()?.parse::<u64>().ok())
                .unwrap_or(1 << attempt);
            let why = if status == 429 { "limite de uso do provedor atingido".to_string() } else { format!("o provedor respondeu {status}") };
            on_frame(Frame::Retry(format!("{why}; nova tentativa em {wait}s ({}/{MAX_RETRIES})", attempt + 1)))?;
            tokio::time::sleep(Duration::from_secs(wait.min(60))).await;
            attempt += 1;
            continue;
        }
        let body = resp.text().await.unwrap_or_default();
        return Err(ProviderError::Api { status, body });
    };

    let limits = rate_limits(resp.headers());
    let mut parser = SseParser::default();
    let mut bytes = resp.bytes_stream();
    while let Some(chunk) = bytes.next().await {
        for (_, data) in parser.push(&chunk?) {
            if !on_frame(Frame::Data(&data))? {
                return Ok(limits);
            }
        }
    }
    Ok(limits)
}
