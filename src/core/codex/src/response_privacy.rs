//! Public Subscription protocol fields. Business content remains opaque.
use serde_json::{Map, Value, json};

use crate::lifecycle_carriers::{CarrierAction, response_header_action};

pub(crate) const FAILURE_MESSAGE: &str = "The upstream request failed.";

pub(crate) fn filter_response(value: &mut Value, request_id: &str) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    filter_container(object, request_id);
    if let Some(response) = object.get_mut("response").and_then(Value::as_object_mut) {
        filter_container(response, request_id);
    }
}

fn filter_container(object: &mut Map<String, Value>, request_id: &str) {
    if let Some(headers) = object.get_mut("headers") {
        filter_headers(headers, request_id);
    }
    if let Some(error) = object.get_mut("error").filter(|value| !value.is_null()) {
        *error = public_error(error.as_object());
    }
    if let Some(details) = object
        .get_mut("incomplete_details")
        .filter(|value| !value.is_null())
    {
        let reason = details
            .get("reason")
            .and_then(Value::as_str)
            .filter(|reason| matches!(*reason, "max_output_tokens" | "content_filter"))
            .unwrap_or("unknown");
        *details = json!({"reason":reason});
    }
    if object.get("type").and_then(Value::as_str) == Some("error") {
        // Responses also permits a flat error event. Keep its envelope/correlation fields,
        // but never provider messages, arbitrary error extensions or diagnostic identifiers.
        if !object.contains_key("error") {
            let error = public_error(Some(object));
            object.insert("code".into(), error["code"].clone());
            object.insert("message".into(), error["message"].clone());
        }
        object.retain(|key, _| {
            matches!(
                key.as_str(),
                "type"
                    | "error"
                    | "code"
                    | "message"
                    | "response_id"
                    | "sequence_number"
                    | "status"
                    | "headers"
            )
        });
        if object.contains_key("error") {
            object.remove("code");
            object.remove("message");
        }
        if object.get("status").is_some_and(|value| {
            value
                .as_u64()
                .is_none_or(|status| !(100..=599).contains(&status))
        }) {
            object.remove("status");
        }
        if object
            .get("sequence_number")
            .is_some_and(|value| value.as_u64().is_none())
        {
            object.remove("sequence_number");
        }
        if object
            .get("response_id")
            .is_some_and(|value| !value.is_string())
        {
            object.remove("response_id");
        }
    }
}

fn filter_headers(value: &mut Value, request_id: &str) {
    let Some(headers) = value.as_object_mut() else {
        *value = json!({});
        return;
    };
    headers.retain(|name, value| match response_header_action(name) {
        CarrierAction::Opaque => header_value(value, None, 0),
        CarrierAction::GatewayRequestAlias if !request_id.is_empty() => {
            header_value(value, Some(request_id), 0)
        }
        _ => false,
    });
}

fn header_value(value: &mut Value, alias: Option<&str>, depth: usize) -> bool {
    match value {
        Value::String(text) => {
            if text.len() > crate::subscription_routing::MAX_ROUTING_TOKEN_BYTES
                || http::HeaderValue::from_str(text).is_err()
            {
                return false;
            }
            if let Some(alias) = alias {
                *text = alias.into();
            }
            true
        }
        Value::Array(values) if depth < 8 => values
            .iter_mut()
            .all(|value| header_value(value, alias, depth + 1)),
        _ => false,
    }
}

fn public_error(object: Option<&Map<String, Value>>) -> Value {
    // Preserve the native client's machine-readable context/retry categories, never an
    // arbitrary provider string masquerading as a code. Unknown codes use the gateway code.
    let field = |name: &str| object.and_then(|object| object.get(name));
    let code = field("code")
        .and_then(Value::as_str)
        .filter(|code| {
            matches!(
                *code,
                "context_length_exceeded"
                    | "insufficient_quota"
                    | "credit_balance_exhausted"
                    | "organization_spend_limit_exceeded"
                    | "organization_usage_limit_exceeded"
                    | "project_spend_limit_exceeded"
                    | "usage_not_included"
                    | "cyber_policy"
                    | "bio_policy"
                    | "misalignment_policy_violation"
                    | "invalid_prompt"
                    | "server_is_overloaded"
                    | "rate_limit_exceeded"
                    | "slow_down"
                    | "previous_response_not_found"
                    | "websocket_connection_limit_reached"
                    | "invalid_request_error"
                    | "server_error"
                    | "internal_error"
                    | "invalid_api_key"
                    | "model_not_found"
                    | "content_policy_violation"
                    | "max_output_tokens"
                    | "content_filter"
            )
        })
        .unwrap_or("upstream_response_failed");
    let mut error = json!({"code": code, "message": FAILURE_MESSAGE});
    if matches!(code, "rate_limit_exceeded" | "slow_down")
        && let Some(delay) = field("message")
            .and_then(Value::as_str)
            .and_then(retry_delay)
    {
        // Native 0.156.0 extracts this delay from text. Retain only the bounded numeric
        // control, never the surrounding provider message or identifying quota details.
        error["message"] = format!("{FAILURE_MESSAGE} Try again in {delay}.").into();
    }
    if let Some(kind) = field("type").and_then(Value::as_str).filter(|kind| {
        matches!(
            *kind,
            "invalid_request_error"
                | "server_error"
                | "authentication_error"
                | "permission_error"
                | "rate_limit_error"
                | "tokens"
                | "requests"
                | "usage_limit_reached"
                | "usage_not_included"
                | "insufficient_quota"
        )
    }) {
        error["type"] = kind.into();
    }
    error
}

fn retry_delay(message: &str) -> Option<String> {
    const PHRASE: &[u8] = b"try again in";
    let start = message
        .as_bytes()
        .windows(PHRASE.len())
        .position(|bytes| bytes.eq_ignore_ascii_case(PHRASE))?;
    let tail = message.get(start + PHRASE.len()..)?;
    let tail = tail.trim_start();
    if !tail.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let length = tail
        .bytes()
        .take_while(|byte| byte.is_ascii_digit() || *byte == b'.')
        .count();
    if length == 0 || length > 20 {
        return None;
    }
    let number: f64 = tail[..length].parse().ok()?;
    let unit = tail[length..].trim_start();
    let (unit, maximum) = if unit
        .get(..2)
        .is_some_and(|unit| unit.eq_ignore_ascii_case("ms"))
    {
        ("ms", 86_400_000.0)
    } else if unit
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.eq_ignore_ascii_case(&b's'))
    {
        ("s", 86_400.0)
    } else {
        return None;
    };
    (number.is_finite() && (0.0..=maximum).contains(&number)).then(|| format!("{number}{unit}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_survives_without_provider_text_or_extensions() {
        let mut value = json!({"type":"error","status":429,"error":{
            "code":"rate_limit_exceeded","message":"synthetic-private quota. Try again in 1250ms. synthetic-private",
            "type":"rate_limit_error","plan":"synthetic-private"}});
        filter_response(&mut value, "req_gateway");
        assert_eq!(
            value["error"]["message"],
            "The upstream request failed. Try again in 1250ms."
        );
        assert_eq!(value["error"]["type"], "rate_limit_error");
        assert_eq!(value["status"], 429);
        assert!(!value.to_string().contains("synthetic-private"));
        assert!(retry_delay("Try again in inf seconds").is_none());
        assert!(retry_delay("Try again in 9999999999999 seconds").is_none());
    }

    #[test]
    fn metadata_uses_the_header_boundary_including_aliases_and_invalid_shapes() {
        let mut value = json!({"type":"response.metadata","headers":{
            "Authorization":"synthetic-private", "Set-Cookie":"synthetic-private",
            "X-Codex-Installation-Id":"synthetic-private", "future-private":"synthetic-private",
            "X-Request-Id":["provider-one","provider-two"],
            "Openai-Request-Id":"provider-three", "x-codex-turn-state":[["routing-state"]],
            "retry-after":"5", "openai-model":{"private":"synthetic-private"},
            "cache-control":"bad\r\nheader"}});
        filter_response(&mut value, "req_gateway");
        assert_eq!(
            value["headers"],
            json!({
            "X-Request-Id":["req_gateway","req_gateway"], "Openai-Request-Id":"req_gateway",
            "x-codex-turn-state":[["routing-state"]], "retry-after":"5"})
        );
        filter_response(&mut value, "");
        assert!(value["headers"].get("X-Request-Id").is_none());
    }

    #[test]
    fn error_normalization_keeps_codes_without_traversing_business_content() {
        let opaque = json!({"error":{"message":"synthetic business text"},"headers":{"authorization":"caller data"}});
        let mut value = json!({"type":"response.failed","response":{
            "id":"resp_public","error":{"code":"context_length_exceeded",
            "message":"synthetic-private","id":"synthetic-private","debug":opaque},
            "metadata":opaque, "output":[{"type":"function_call","arguments":opaque.to_string()}]}});
        filter_response(&mut value, "req_gateway");
        assert_eq!(
            value["response"]["error"],
            json!({"code":"context_length_exceeded","message":FAILURE_MESSAGE})
        );
        let mut incomplete = json!({"type":"response.incomplete","response":{
            "incomplete_details":{"reason":"synthetic-private","debug":"synthetic-private"}}});
        filter_response(&mut incomplete, "req_gateway");
        assert_eq!(
            incomplete["response"]["incomplete_details"],
            json!({"reason":"unknown"})
        );
        incomplete["response"]["incomplete_details"] =
            json!({"reason":"max_output_tokens","debug":"synthetic-private"});
        filter_response(&mut incomplete, "req_gateway");
        assert_eq!(
            incomplete["response"]["incomplete_details"],
            json!({"reason":"max_output_tokens"})
        );
        assert_eq!(value["response"]["metadata"], opaque);
        assert_eq!(
            value["response"]["output"][0]["arguments"],
            opaque.to_string()
        );
        let mut flat = json!({"type":"error","response_id":"resp_public","sequence_number":1,
            "code":"synthetic-private","message":"synthetic-private","debug":"synthetic-private"});
        filter_response(&mut flat, "req_gateway");
        assert_eq!(
            flat,
            json!({"type":"error","response_id":"resp_public","sequence_number":1,
            "code":"upstream_response_failed","message":FAILURE_MESSAGE})
        );
    }
}
