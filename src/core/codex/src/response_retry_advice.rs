//! Bounded native Retry-After parsing; publishing advice never schedules gateway retries.
use http::HeaderValue;
use serde_json::{Map, Value};
use std::time::{Duration, Instant};

pub(crate) fn retry_after_header(value: &HeaderValue) -> Option<HeaderValue> {
    let raw = value.to_str().ok()?;
    if raw.len() > crate::subscription_routing::MAX_ROUTING_TOKEN_BYTES {
        return None;
    }
    let text = raw.trim();
    let valid = if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
        text.parse::<u64>()
            .ok()
            .and_then(|seconds| Instant::now().checked_add(Duration::from_secs(seconds)))
            .is_some()
    } else {
        httpdate::parse_http_date(text).is_ok()
    };
    valid.then(|| value.clone())
}

pub(super) fn from_json(headers: &Map<String, Value>) -> Option<HeaderValue> {
    let mut effective = None;
    // Native inserts each syntactically valid scalar into a case-insensitive HeaderMap.
    // A later parse-invalid scalar overrides an earlier delay; a container does not.
    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("retry-after") {
            continue;
        }
        let value = match value {
            Value::String(value) => {
                if value.len() > crate::subscription_routing::MAX_ROUTING_TOKEN_BYTES {
                    effective = None;
                    continue;
                }
                value.clone()
            }
            Value::Number(_) | Value::Bool(_) => value.to_string(),
            _ => continue,
        };
        if let Ok(value) = HeaderValue::from_str(&value) {
            effective = Some(value);
        }
    }
    effective.as_ref().and_then(retry_after_header)
}

pub(super) fn replace(headers: &mut Map<String, Value>, value: Option<HeaderValue>) {
    headers.retain(|name, _| !name.eq_ignore_ascii_case("retry-after"));
    if let Some(value) = value.and_then(|value| value.to_str().ok().map(str::to_owned)) {
        headers.insert("retry-after".into(), value.into());
    }
}
