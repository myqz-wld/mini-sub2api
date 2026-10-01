//! Recognize the pinned native terminal category without exposing upstream error text.
use crate::error::CoreFailure;
use crate::error::NativeErrorCategory;
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

#[cfg(test)]
#[path = "response_usage_window_tests.rs"]
mod usage_window_tests;

#[derive(Deserialize)]
struct ErrorCode {
    code: Option<String>,
    #[serde(rename = "type")]
    kind: Option<Value>,
    limit_window_minutes: Option<Value>,
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
        "error" => event.error.and_then(|error| error.code).or(event.code),
        "response.failed" => event.response.and_then(|response| response.error.code),
        _ => None,
    };
    code.as_deref() == Some("flex_unavailable")
}

pub(crate) fn http_category(status: http::StatusCode, bytes: &[u8]) -> Option<CoreFailure> {
    if bytes.len() > 64 * 1024 {
        return None;
    }
    let body = serde_json::from_slice::<FailureBody>(bytes).ok()?;
    let code = body.error.code.as_deref().unwrap_or_default();
    match (status, code) {
        (http::StatusCode::TOO_MANY_REQUESTS, "flex_unavailable") => {
            Some(CoreFailure::FlexUnavailable)
        }
        (http::StatusCode::BAD_REQUEST, "invalid_prompt") => {
            Some(CoreFailure::UpstreamInvalidPrompt)
        }
        _ => {
            use NativeErrorCategory as Native;
            let category = match (
                status.as_u16(),
                code,
                body.error.kind.as_ref().and_then(Value::as_str),
            ) {
                (503, "server_is_overloaded", _) => Native::ServerOverloaded,
                (503, "slow_down", _) => Native::SlowDown,
                (400 | 403, "misalignment_policy_violation", _) => Native::MisalignmentPolicy,
                (400, "cyber_policy", _) => Native::CyberPolicy,
                (400, "bio_policy", _) => Native::BioPolicy,
                (429, _, Some("usage_limit_reached")) => {
                    Native::UsageLimitReached(usage_limit_window(
                        Some("usage_limit_reached"),
                        body.error.limit_window_minutes.as_ref(),
                    ))
                }
                (429, _, Some("usage_not_included")) => Native::UsageNotIncluded,
                (429, _, Some("insufficient_quota"))
                | (
                    429,
                    "insufficient_quota"
                    | "credit_balance_exhausted"
                    | "organization_spend_limit_exceeded"
                    | "project_spend_limit_exceeded"
                    | "organization_usage_limit_exceeded",
                    _,
                ) => Native::QuotaExceeded,
                _ => return None,
            };
            Some(CoreFailure::NativeResponse(category, status))
        }
    }
}

pub(crate) fn usage_limit_window(kind: Option<&str>, value: Option<&Value>) -> Option<u16> {
    if kind != Some("usage_limit_reached") {
        return None;
    }
    value
        .and_then(Value::as_u64)
        .and_then(|minutes| u16::try_from(minutes).ok())
}

pub(crate) fn retry_metadata(bytes: &[u8]) -> Option<http::HeaderValue> {
    if bytes.len() > 64 * 1024 {
        return None;
    }
    let body: Value = serde_json::from_slice(bytes).ok()?;
    let headers = body.pointer("/error/headers")?.as_object()?;
    let (_, value) = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-retry-metadata"))?;
    let value = value.as_str()?;
    if value.len() > crate::subscription_routing::MAX_ROUTING_TOKEN_BYTES {
        return None;
    }
    http::HeaderValue::from_str(value).ok()
}

// A bounded, separately timed read only for native categorized rejection statuses. Other
// failures keep the established content-free response and never read their body.
pub(super) async fn classify_http(
    upstream: reqwest::Response,
) -> (CoreFailure, Option<http::HeaderValue>) {
    const MAXIMUM: usize = 64 * 1024;
    let status = upstream.status();
    if !matches!(
        status,
        http::StatusCode::TOO_MANY_REQUESTS
            | http::StatusCode::BAD_REQUEST
            | http::StatusCode::FORBIDDEN
            | http::StatusCode::SERVICE_UNAVAILABLE
    ) || upstream
        .content_length()
        .is_some_and(|n| n > MAXIMUM as u64)
    {
        return (CoreFailure::UpstreamResponseFailed, None);
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
        Some((
            http_category(status, &bytes).unwrap_or(CoreFailure::UpstreamResponseFailed),
            retry_metadata(&bytes),
        ))
    })
    .await;
    body.ok()
        .flatten()
        .unwrap_or((CoreFailure::UpstreamResponseFailed, None))
}
