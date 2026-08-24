use crate::request_defaults;
use crate::request_defaults::ModelProfile;
use crate::request_identity;
use crate::request_identity::IdentityContext;
use crate::request_identity::SubscriptionTransport;
use crate::responses_lite;
use bytes::Bytes;
use http::HeaderMap;
use serde_json::Map;
use serde_json::Value;

const UNSUPPORTED_SUBSCRIPTION_BODY_FIELDS: &[&str] = &[
    "max_output_tokens",
    "max_completion_tokens",
    "max_tokens",
    "temperature",
    "top_p",
    "frequency_penalty",
    "presence_penalty",
];

pub struct PreparedSubscriptionRequest {
    pub headers: HeaderMap,
    pub body: Bytes,
}

pub fn prepare_subscription_request(
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    installation_id: &str,
    request_id: &str,
) -> PreparedSubscriptionRequest {
    prepare_subscription_request_for(
        headers,
        body,
        max_bytes,
        installation_id,
        request_id,
        SubscriptionTransport::Http,
    )
}

pub fn prepare_websocket_subscription_request(
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    installation_id: &str,
    request_id: &str,
) -> PreparedSubscriptionRequest {
    prepare_subscription_request_for(
        headers,
        body,
        max_bytes,
        installation_id,
        request_id,
        SubscriptionTransport::WebSocket,
    )
}

fn prepare_subscription_request_for(
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    installation_id: &str,
    _request_id: &str,
    transport: SubscriptionTransport,
) -> PreparedSubscriptionRequest {
    let mut prepared_headers = headers.clone();
    if has_non_identity_encoding(headers) {
        return prepared(prepared_headers, body);
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(&body) else {
        return prepared(prepared_headers, body);
    };
    let Some(object) = value.as_object_mut() else {
        return prepared(prepared_headers, body);
    };
    let changed = strip_unsupported_subscription_fields(object)
        | request_defaults::normalize_optional_members(object);
    let filtered_body = changed
        .then(|| serde_json::to_vec(&value).ok())
        .flatten()
        .filter(|encoded| encoded.len() <= max_bytes)
        .map(Bytes::from);
    let object = value
        .as_object_mut()
        .expect("the request object was validated above");
    if object
        .get("instructions")
        .and_then(Value::as_str)
        .is_some_and(str::is_empty)
    {
        object.remove("instructions");
    }
    let mut profile = object
        .get("model")
        .and_then(Value::as_str)
        .map(request_defaults::model_profile)
        .unwrap_or_else(|| request_defaults::model_profile(""));
    profile.responses_lite |= responses_lite_requested(object);
    request_identity::apply_routing_hint(object, &mut prepared_headers);
    if already_subscription_shaped(object, profile) {
        request_defaults::merge_request_defaults(object, profile);
        request_identity::apply(
            object,
            &mut prepared_headers,
            IdentityContext {
                installation_id,
                responses_lite: profile.responses_lite,
                transport,
                tool_namespaces_info: None,
            },
        );
        responses_lite::canonicalize_request_items(object);
        canonicalize_request_order(object, transport);
        return encode_prepared(
            prepared_headers,
            &value,
            max_bytes,
            filtered_body.unwrap_or(body),
        );
    }

    let Some(input_value) = object.remove("input") else {
        return prepared(prepared_headers, filtered_body.unwrap_or(body));
    };
    let mut input = match normalize_input(input_value) {
        Some(input) => input,
        None => return prepared(prepared_headers, filtered_body.unwrap_or(body)),
    };
    let tools = match object.remove("tools") {
        Some(Value::Array(tools)) => tools,
        Some(other) => {
            object.insert("tools".to_string(), other);
            return prepared(prepared_headers, filtered_body.unwrap_or(body));
        }
        None => Vec::new(),
    };
    let instructions = match object.remove("instructions") {
        Some(Value::String(instructions)) => instructions,
        Some(other) => {
            object.insert("instructions".to_string(), other);
            return prepared(prepared_headers, filtered_body.unwrap_or(body));
        }
        None => String::new(),
    };

    let tools = if profile.responses_lite {
        responses_lite::group_tools(tools)
    } else {
        responses_lite::canonicalize_tools(tools)
    };
    if profile.responses_lite {
        let mut prefix = vec![serde_json::json!({
            "type": "additional_tools",
            "role": "developer",
            "tools": tools,
        })];
        if !instructions.is_empty() {
            prefix.push(developer_message(instructions));
        }
        prefix.append(&mut input);
        object.insert("input".to_string(), Value::Array(prefix));
        object.insert("parallel_tool_calls".to_string(), Value::Bool(false));
    } else {
        object.insert("input".to_string(), Value::Array(input));
        object.insert("tools".to_string(), Value::Array(tools));
        if !instructions.is_empty() {
            object.insert("instructions".to_string(), Value::String(instructions));
        }
    }
    request_defaults::merge_request_defaults(object, profile);
    request_identity::apply(
        object,
        &mut prepared_headers,
        IdentityContext {
            installation_id,
            responses_lite: profile.responses_lite,
            transport,
            tool_namespaces_info: None,
        },
    );
    responses_lite::canonicalize_request_items(object);
    canonicalize_request_order(object, transport);

    let Ok(encoded) = serde_json::to_vec(&value) else {
        return prepared(headers.clone(), filtered_body.unwrap_or(body));
    };
    if encoded.len() > max_bytes {
        return prepared(headers.clone(), filtered_body.unwrap_or(body));
    }
    prepared(prepared_headers, Bytes::from(encoded))
}

fn encode_prepared(
    headers: HeaderMap,
    value: &Value,
    max_bytes: usize,
    fallback: Bytes,
) -> PreparedSubscriptionRequest {
    match serde_json::to_vec(value) {
        Ok(encoded) if encoded.len() <= max_bytes => prepared(headers, Bytes::from(encoded)),
        _ => prepared(headers, fallback),
    }
}

fn canonicalize_request_order(object: &mut Map<String, Value>, transport: SubscriptionTransport) {
    const HTTP_ORDER: &[&str] = &[
        "model",
        "instructions",
        "input",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "reasoning",
        "store",
        "stream",
        "stream_options",
        "include",
        "service_tier",
        "prompt_cache_key",
        "text",
        "client_metadata",
    ];
    const WEBSOCKET_ORDER: &[&str] = &[
        "type",
        "model",
        "instructions",
        "previous_response_id",
        "input",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "reasoning",
        "store",
        "stream",
        "stream_options",
        "include",
        "service_tier",
        "prompt_cache_key",
        "text",
        "generate",
        "client_metadata",
    ];
    let order = if transport == SubscriptionTransport::WebSocket {
        WEBSOCKET_ORDER
    } else {
        HTTP_ORDER
    };
    let mut existing = std::mem::take(object);
    for name in order {
        if let Some(value) = existing.remove(*name) {
            object.insert((*name).to_string(), value);
        }
    }
}

fn strip_unsupported_subscription_fields(object: &mut Map<String, Value>) -> bool {
    let mut removed = false;
    for field in UNSUPPORTED_SUBSCRIPTION_BODY_FIELDS {
        removed |= object.remove(*field).is_some();
    }
    removed
}

fn prepared(headers: HeaderMap, body: Bytes) -> PreparedSubscriptionRequest {
    PreparedSubscriptionRequest { headers, body }
}

fn has_non_identity_encoding(headers: &HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let value = value.trim();
            !value.is_empty() && !value.eq_ignore_ascii_case("identity")
        })
}

fn already_subscription_shaped(object: &Map<String, Value>, profile: ModelProfile) -> bool {
    let input = object.get("input").and_then(Value::as_array);
    if profile.responses_lite {
        return object.get("tools").is_none()
            && object.get("instructions").is_none()
            && (input
                .and_then(|items| items.first())
                .and_then(Value::as_object)
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
                == Some("additional_tools")
                || object
                    .get("previous_response_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty()));
    }
    input.is_some()
        && object.get("tools").is_some_and(Value::is_array)
        && [
            "tool_choice",
            "parallel_tool_calls",
            "reasoning",
            "store",
            "stream",
            "include",
            "prompt_cache_key",
            "client_metadata",
        ]
        .iter()
        .all(|field| object.contains_key(*field))
}

fn normalize_input(input: Value) -> Option<Vec<Value>> {
    let mut input = match input {
        Value::String(text) => Some(vec![user_message(text)]),
        Value::Array(input) => Some(input),
        _ => None,
    }?;
    for item in &mut input {
        let Some(message) = item.as_object_mut() else {
            continue;
        };
        if message.get("type").is_none() && message.get("role").and_then(Value::as_str).is_some() {
            let existing = std::mem::take(message);
            message.insert("type".to_string(), Value::String("message".to_string()));
            message.extend(existing);
        }
        if message.get("role").and_then(Value::as_str) == Some("system") {
            message.insert("role".to_string(), Value::String("developer".to_string()));
        }
        if matches!(
            message.get("type").and_then(Value::as_str),
            None | Some("message")
        ) && let Some(text) = message.get("content").and_then(Value::as_str)
        {
            message.insert(
                "content".to_string(),
                serde_json::json!([{"type": "input_text", "text": text}]),
            );
        }
    }
    responses_lite::assign_missing_item_ids(&mut input);
    Some(input)
}

fn developer_message(text: String) -> Value {
    serde_json::json!({
        "type": "message",
        "role": "developer",
        "content": [{"type": "input_text", "text": text}],
    })
}

fn responses_lite_requested(object: &Map<String, Value>) -> bool {
    object
        .get("input")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_object)
        .and_then(|item| item.get("type"))
        .and_then(Value::as_str)
        == Some("additional_tools")
}

fn user_message(text: String) -> Value {
    serde_json::json!({
        "type": "message",
        "id": responses_lite::new_item_id("msg"),
        "role": "user",
        "content": [{"type": "input_text", "text": text}],
    })
}

#[cfg(test)]
fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_string)
}

#[cfg(test)]
#[path = "request_normalizer_tests.rs"]
mod tests;
