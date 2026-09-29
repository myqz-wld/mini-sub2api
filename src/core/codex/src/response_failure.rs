//! Recognize the pinned native terminal category without exposing upstream error text.
use crate::error::CoreFailure;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

pub(crate) fn is_flex_event(value: &Value) -> bool {
    let error = match value.get("type").and_then(Value::as_str) {
        Some("error") => value
            .get("error")
            .filter(|error| !error.is_null())
            .unwrap_or(value),
        Some("response.failed") => value.pointer("/response/error").unwrap_or(&Value::Null),
        _ => return false,
    };
    error.get("code").and_then(Value::as_str) == Some("flex_unavailable")
}

#[cfg(test)]
#[path = "response_failure_tests.rs"]
mod tests;

#[derive(Deserialize)]
struct ErrorCode {
    code: String,
}

#[derive(Deserialize)]
struct FailureBody {
    error: ErrorCode,
}

#[derive(Deserialize)]
struct FailureEvent {
    #[serde(rename = "type")]
    kind: String,
    error: Option<ErrorCode>,
    code: Option<String>,
    response: Option<FailureBody>,
}

pub(crate) fn is_flex_sse(data: &str) -> bool {
    let Ok(event) = serde_json::from_str::<FailureEvent>(data) else {
        return false;
    };
    let code = match event.kind.as_str() {
        "error" => event.error.map(|error| error.code).or(event.code),
        "response.failed" => event.response.map(|response| response.error.code),
        _ => None,
    };
    code.as_deref() == Some("flex_unavailable")
}

pub(crate) fn http_category(status: http::StatusCode, bytes: &[u8]) -> Option<CoreFailure> {
    if bytes.len() > 64 * 1024 {
        return None;
    }
    let body = serde_json::from_slice::<FailureBody>(bytes).ok()?;
    match (status, body.error.code.as_str()) {
        (http::StatusCode::TOO_MANY_REQUESTS, "flex_unavailable") => {
            Some(CoreFailure::FlexUnavailable)
        }
        (http::StatusCode::BAD_REQUEST, "invalid_prompt") => {
            Some(CoreFailure::UpstreamInvalidPrompt)
        }
        _ => None,
    }
}

// A bounded, separately timed read only for the new native 400/429 categories. Other
// failures keep the established content-free response and never read their body.
pub(super) async fn classify_http(upstream: reqwest::Response) -> CoreFailure {
    const MAXIMUM: usize = 64 * 1024;
    let status = upstream.status();
    if !matches!(
        status,
        http::StatusCode::TOO_MANY_REQUESTS | http::StatusCode::BAD_REQUEST
    ) || upstream
        .content_length()
        .is_some_and(|n| n > MAXIMUM as u64)
    {
        return CoreFailure::UpstreamResponseFailed;
    }
    let body = tokio::time::timeout(Duration::from_secs(1), async {
        let mut stream = upstream.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.ok()?;
            if chunk.len() > MAXIMUM.saturating_sub(bytes.len()) {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        http_category(status, &bytes)
    })
    .await;
    body.ok()
        .flatten()
        .unwrap_or(CoreFailure::UpstreamResponseFailed)
}
