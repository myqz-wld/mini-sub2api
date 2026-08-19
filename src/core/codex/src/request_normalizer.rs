use bytes::Bytes;
use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use serde_json::Map;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use uuid::Uuid;

pub struct PreparedSubscriptionRequest {
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Clone, Copy)]
struct ModelProfile {
    responses_lite: bool,
    reasoning_effort: &'static str,
    verbosity: &'static str,
}

struct RequestIdentity {
    session_id: String,
    thread_id: String,
    installation_id: String,
    turn_id: String,
    window_id: String,
    turn_metadata: String,
}

pub fn prepare_subscription_request(
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    account_ref: &str,
    request_id: &str,
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
    let profile = object
        .get("model")
        .and_then(Value::as_str)
        .map(model_profile)
        .unwrap_or_else(|| model_profile(""));
    if already_subscription_shaped(object, profile) {
        let identity = resolve_identity(object, headers, account_ref, request_id);
        apply_identity_headers(&mut prepared_headers, profile, &identity);
        return prepared(prepared_headers, body);
    }

    let Some(input_value) = object.remove("input") else {
        return prepared(prepared_headers, body);
    };
    let mut input = match normalize_input(input_value) {
        Some(input) => input,
        None => return prepared(prepared_headers, body),
    };
    let tools = match object.remove("tools") {
        Some(Value::Array(tools)) => tools,
        Some(other) => {
            object.insert("tools".to_string(), other);
            return prepared(prepared_headers, body);
        }
        None => Vec::new(),
    };
    let instructions = match object.remove("instructions") {
        Some(Value::String(instructions)) => instructions,
        Some(other) => {
            object.insert("instructions".to_string(), other);
            return prepared(prepared_headers, body);
        }
        None => String::new(),
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
        object
            .entry("parallel_tool_calls".to_string())
            .or_insert(Value::Bool(true));
    }
    object.insert("store".to_string(), Value::Bool(false));
    object
        .entry("stream".to_string())
        .or_insert(Value::Bool(true));
    object
        .entry("tool_choice".to_string())
        .or_insert_with(|| Value::String("auto".to_string()));
    merge_reasoning(object, profile);
    merge_text(object, profile);
    merge_include(object);
    merge_metadata(
        object,
        &mut prepared_headers,
        profile,
        account_ref,
        request_id,
    );

    let Ok(encoded) = serde_json::to_vec(&value) else {
        return prepared(headers.clone(), body);
    };
    if encoded.len() > max_bytes {
        return prepared(headers.clone(), body);
    }
    prepared(prepared_headers, Bytes::from(encoded))
}

fn prepared(headers: HeaderMap, body: Bytes) -> PreparedSubscriptionRequest {
    PreparedSubscriptionRequest { headers, body }
}

fn model_profile(model: &str) -> ModelProfile {
    match model {
        "gpt-5.6-sol" => ModelProfile {
            responses_lite: true,
            reasoning_effort: "low",
            verbosity: "low",
        },
        "gpt-5.6-terra" | "gpt-5.6-luna" => ModelProfile {
            responses_lite: true,
            reasoning_effort: "medium",
            verbosity: "low",
        },
        "gpt-5.4-mini" => ModelProfile {
            responses_lite: false,
            reasoning_effort: "medium",
            verbosity: "medium",
        },
        _ => ModelProfile {
            responses_lite: false,
            reasoning_effort: "medium",
            verbosity: "low",
        },
    }
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
            && input
                .and_then(|items| items.first())
                .and_then(Value::as_object)
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
                == Some("additional_tools");
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
    match input {
        Value::String(text) => Some(vec![user_message(text)]),
        Value::Array(input) => Some(input),
        _ => None,
    }
}

fn developer_message(text: String) -> Value {
    serde_json::json!({
        "type": "message",
        "role": "developer",
        "content": [{"type": "input_text", "text": text}],
    })
}

fn user_message(text: String) -> Value {
    serde_json::json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": text}],
    })
}

fn merge_reasoning(object: &mut Map<String, Value>, profile: ModelProfile) {
    let reasoning = object
        .entry("reasoning".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(reasoning) = reasoning.as_object_mut() else {
        return;
    };
    reasoning
        .entry("effort".to_string())
        .or_insert_with(|| Value::String(profile.reasoning_effort.to_string()));
    if profile.responses_lite {
        reasoning
            .entry("context".to_string())
            .or_insert_with(|| Value::String("all_turns".to_string()));
    }
}

fn merge_text(object: &mut Map<String, Value>, profile: ModelProfile) {
    let text = object
        .entry("text".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(text) = text.as_object_mut() else {
        return;
    };
    text.entry("verbosity".to_string())
        .or_insert_with(|| Value::String(profile.verbosity.to_string()));
}

fn merge_include(object: &mut Map<String, Value>) {
    match object.get_mut("include") {
        Some(Value::Array(include)) => {
            if !include
                .iter()
                .any(|item| item.as_str() == Some("reasoning.encrypted_content"))
            {
                include.push(Value::String("reasoning.encrypted_content".to_string()));
            }
        }
        None => {
            object.insert(
                "include".to_string(),
                serde_json::json!(["reasoning.encrypted_content"]),
            );
        }
        Some(_) => {}
    }
}

fn merge_metadata(
    object: &mut Map<String, Value>,
    headers: &mut HeaderMap,
    profile: ModelProfile,
    account_ref: &str,
    request_id: &str,
) {
    let identity = resolve_identity(object, headers, account_ref, request_id);
    apply_identity_headers(headers, profile, &identity);

    let metadata = object
        .entry("client_metadata".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(metadata) = metadata.as_object_mut() {
        for (name, value) in [
            ("session_id", identity.session_id.as_str()),
            ("thread_id", identity.thread_id.as_str()),
            ("turn_id", identity.turn_id.as_str()),
            ("x-codex-installation-id", identity.installation_id.as_str()),
            ("x-codex-turn-metadata", identity.turn_metadata.as_str()),
            ("x-codex-window-id", identity.window_id.as_str()),
        ] {
            metadata
                .entry(name.to_string())
                .or_insert_with(|| Value::String(value.to_string()));
        }
    }
    object
        .entry("prompt_cache_key".to_string())
        .or_insert_with(|| Value::String(identity.session_id));
}

fn resolve_identity(
    object: &Map<String, Value>,
    headers: &HeaderMap,
    account_ref: &str,
    request_id: &str,
) -> RequestIdentity {
    let session_id = header_text(headers, "session-id")
        .or_else(|| client_metadata_text(object, "session_id"))
        .unwrap_or_else(|| derived_uuid("session", request_id));
    let thread_id = header_text(headers, "thread-id")
        .or_else(|| client_metadata_text(object, "thread_id"))
        .unwrap_or_else(|| session_id.clone());
    let installation_id = header_text(headers, "x-codex-installation-id")
        .or_else(|| client_metadata_text(object, "x-codex-installation-id"))
        .unwrap_or_else(|| derived_uuid("installation", account_ref));
    let existing_turn_metadata = header_text(headers, "x-codex-turn-metadata")
        .or_else(|| client_metadata_text(object, "x-codex-turn-metadata"));
    let turn_id = existing_turn_metadata
        .as_deref()
        .and_then(turn_id_from_metadata)
        .or_else(|| client_metadata_text(object, "turn_id"))
        .or_else(|| header_text(headers, "x-client-request-id"))
        .unwrap_or_else(|| derived_uuid("turn", request_id));
    let window_id = header_text(headers, "x-codex-window-id")
        .or_else(|| client_metadata_text(object, "x-codex-window-id"))
        .unwrap_or_else(|| derived_uuid("window", request_id));
    let turn_metadata = existing_turn_metadata.unwrap_or_else(|| {
        serde_json::json!({
            "installation_id": installation_id,
            "session_id": session_id,
            "thread_id": thread_id,
            "turn_id": turn_id,
            "window_id": window_id,
            "request_kind": "turn",
        })
        .to_string()
    });
    RequestIdentity {
        session_id,
        thread_id,
        installation_id,
        turn_id,
        window_id,
        turn_metadata,
    }
}

fn apply_identity_headers(
    headers: &mut HeaderMap,
    profile: ModelProfile,
    identity: &RequestIdentity,
) {
    for (name, value) in [
        ("session-id", identity.session_id.as_str()),
        ("thread-id", identity.thread_id.as_str()),
        ("x-client-request-id", identity.turn_id.as_str()),
        ("x-codex-installation-id", identity.installation_id.as_str()),
        ("x-codex-turn-metadata", identity.turn_metadata.as_str()),
        ("x-codex-window-id", identity.window_id.as_str()),
    ] {
        insert_header_if_missing(headers, name, value);
    }
    ensure_lite_header(headers, profile.responses_lite);
}

fn client_metadata_text(object: &Map<String, Value>, name: &str) -> Option<String> {
    object
        .get("client_metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get(name))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn turn_id_from_metadata(raw: &str) -> Option<String> {
    serde_json::from_str::<Value>(raw).ok().and_then(|value| {
        value
            .get("turn_id")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn ensure_lite_header(headers: &mut HeaderMap, responses_lite: bool) {
    if responses_lite {
        headers.insert(
            HeaderName::from_static("x-openai-internal-codex-responses-lite"),
            HeaderValue::from_static("true"),
        );
    } else {
        headers.remove("x-openai-internal-codex-responses-lite");
    }
}

fn insert_header_if_missing(headers: &mut HeaderMap, name: &'static str, value: &str) {
    let name = HeaderName::from_static(name);
    if headers.contains_key(&name) {
        return;
    }
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

fn derived_uuid(scope: &str, seed: &str) -> String {
    let digest = Sha256::digest(format!("{scope}:{seed}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).to_string()
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

#[cfg(test)]
#[path = "request_normalizer_tests.rs"]
mod tests;
