use super::EmulationTransport;
use crate::codex_instructions;
use crate::request_defaults;
use crate::request_identity;
use crate::request_identity::IdentityContext;
use crate::request_profile::UpstreamProfile;
use crate::responses_lite;
use http::HeaderMap;
use serde_json::Map;
use serde_json::Value;

// Transport-neutral documented Responses create fields plus the captured Codex
// `client_metadata` carrier. HTTP- and WebSocket-only fields are selected separately below.
const SUPPORTED_REQUEST_FIELDS: &[&str] = &[
    "client_metadata",
    "context_management",
    "conversation",
    "include",
    "input",
    "instructions",
    "max_output_tokens",
    "max_tool_calls",
    "metadata",
    "model",
    "moderation",
    "parallel_tool_calls",
    "previous_response_id",
    "prompt",
    "prompt_cache_key",
    "prompt_cache_options",
    "prompt_cache_retention",
    "reasoning",
    "safety_identifier",
    "service_tier",
    "store",
    "stream_options",
    "temperature",
    "text",
    "tool_choice",
    "tools",
    "top_logprobs",
    "top_p",
    "truncation",
    "user",
];

const SUPPORTED_HTTP_FIELDS: &[&str] = &["background", "stream"];
const SUPPORTED_WEBSOCKET_FIELDS: &[&str] = &["type", "generate", "stream_id"];
// Codex 0.149.0 does not expose these public Responses fields in its request builder.
const UNSUPPORTED_CODEX_EMULATION_FIELDS: &[&str] = &[
    "metadata",
    "prompt_cache_retention",
    "safety_identifier",
    "truncation",
    "user",
];
const UNSUPPORTED_SUBSCRIPTION_FIELDS: &[&str] = &[
    "max_output_tokens",
    "max_completion_tokens",
    "max_tokens",
    "temperature",
    "top_p",
    "frequency_penalty",
    "presence_penalty",
    "stream_options",
];

pub(super) fn apply(
    object: &mut Map<String, Value>,
    headers: &mut HeaderMap,
    transport: EmulationTransport,
    profile: UpstreamProfile,
) -> Result<Vec<String>, ()> {
    object.retain(|name, _| {
        SUPPORTED_REQUEST_FIELDS.contains(&name.as_str())
            || match transport {
                EmulationTransport::Http => SUPPORTED_HTTP_FIELDS.contains(&name.as_str()),
                EmulationTransport::WebSocket => {
                    SUPPORTED_WEBSOCKET_FIELDS.contains(&name.as_str())
                }
            }
    });
    strip_unsupported_codex_emulation_fields(object);
    canonicalize_structured_request_members(object);
    let mut model_profile = object
        .get("model")
        .and_then(Value::as_str)
        .map(request_defaults::model_profile)
        .unwrap_or_else(|| request_defaults::model_profile(""));
    model_profile.responses_lite |= responses_lite_requested(object);
    let lite_incremental = model_profile.responses_lite && lite_incremental(object, transport);
    let already_lite =
        model_profile.responses_lite && (responses_lite_requested(object) || lite_incremental);

    let synthesized_item_ids = if already_lite {
        Vec::new()
    } else {
        normalize_input(object)
    };
    codex_instructions::apply(object, model_profile.responses_lite, lite_incremental)?;
    if model_profile.responses_lite {
        if !already_lite {
            relocate_lite_tools(object);
        }
    } else {
        canonicalize_top_level_tools(object);
    }
    if profile.uses_codex_subscription() {
        strip_unsupported_subscription_fields(object);
        rewrite_subscription_system_roles(object);
    }

    request_defaults::merge_request_defaults(
        object,
        model_profile,
        transport == EmulationTransport::Http,
    );
    enforce_upstream_transport_controls(object, transport);
    if profile.uses_codex_subscription() {
        request_identity::apply_routing_hint(object, headers);
    } else {
        request_identity::remove_routing_hint(headers);
    }
    request_identity::apply(
        object,
        headers,
        IdentityContext {
            responses_lite: model_profile.responses_lite,
            transport,
            tool_namespaces_info: None,
        },
    );
    responses_lite::canonicalize_request_items(
        object,
        (!model_profile.responses_lite).then_some("high"),
    );
    canonicalize_request_order(object, transport);
    Ok(synthesized_item_ids)
}

fn strip_unsupported_codex_emulation_fields(object: &mut Map<String, Value>) {
    for field in UNSUPPORTED_CODEX_EMULATION_FIELDS {
        object.remove(*field);
    }
}

fn enforce_upstream_transport_controls(
    object: &mut Map<String, Value>,
    transport: EmulationTransport,
) {
    object.insert("store".to_string(), Value::Bool(false));
    if transport == EmulationTransport::Http {
        object.insert("stream".to_string(), Value::Bool(true));
    }
}

fn canonicalize_structured_request_members(object: &mut Map<String, Value>) {
    retain_object_member(object, "conversation", &["id"]);
    retain_object_member(object, "moderation", &["model", "policy"]);
    if let Some(policy) = object
        .get_mut("moderation")
        .and_then(Value::as_object_mut)
        .and_then(|moderation| moderation.get_mut("policy"))
        .and_then(Value::as_object_mut)
    {
        policy.retain(|name, _| matches!(name.as_str(), "input" | "output"));
        for value in policy.values_mut() {
            if let Some(mode) = value.as_object_mut() {
                mode.retain(|name, _| name == "mode");
            }
        }
    }
    retain_object_member(object, "prompt", &["id", "variables", "version"]);
    retain_object_member(object, "prompt_cache_options", &["mode", "ttl"]);
    if let Some(entries) = object
        .get_mut("context_management")
        .and_then(Value::as_array_mut)
    {
        for entry in entries {
            if let Some(entry) = entry.as_object_mut() {
                entry.retain(|name, _| matches!(name.as_str(), "type" | "compact_threshold"));
            }
        }
    } else if let Some(entry) = object
        .get_mut("context_management")
        .and_then(Value::as_object_mut)
    {
        entry.retain(|name, _| matches!(name.as_str(), "type" | "compact_threshold"));
    }
    let Some(tool_choice) = object.get_mut("tool_choice").and_then(Value::as_object_mut) else {
        return;
    };
    tool_choice.retain(|name, _| {
        matches!(
            name.as_str(),
            "type" | "name" | "server_label" | "mode" | "tools"
        )
    });
    if let Some(tools) = tool_choice.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            if let Some(tool) = tool.as_object_mut() {
                tool.retain(|name, _| matches!(name.as_str(), "type" | "name" | "server_label"));
            }
        }
    }
}

fn retain_object_member(object: &mut Map<String, Value>, name: &str, fields: &[&str]) {
    if let Some(member) = object.get_mut(name).and_then(Value::as_object_mut) {
        member.retain(|name, _| fields.contains(&name.as_str()));
    }
}

fn relocate_lite_tools(object: &mut Map<String, Value>) {
    let Some(mut input) = object.get("input").and_then(Value::as_array).cloned() else {
        return;
    };
    let tools = match object.get("tools") {
        Some(Value::Array(tools)) => tools.clone(),
        None => Vec::new(),
        Some(_) => return,
    };
    object.remove("tools");
    let mut relocated = vec![serde_json::json!({
        "type": "additional_tools",
        "role": "developer",
        "tools": responses_lite::group_tools(tools),
    })];
    relocated.append(&mut input);
    object.insert("input".to_string(), Value::Array(relocated));
}

fn canonicalize_top_level_tools(object: &mut Map<String, Value>) {
    let Some(tools) = object.get("tools").and_then(Value::as_array).cloned() else {
        return;
    };
    *object.get_mut("tools").expect("tools member exists") =
        Value::Array(responses_lite::canonicalize_tools(tools));
}

fn normalize_input(object: &mut Map<String, Value>) -> Vec<String> {
    let Some(input) = object.get_mut("input") else {
        return Vec::new();
    };
    let mut items = match std::mem::take(input) {
        Value::String(text) => vec![user_message(text)],
        Value::Array(items) => items,
        other => {
            *input = other;
            return Vec::new();
        }
    };
    for item in &mut items {
        let Some(message) = item.as_object_mut() else {
            continue;
        };
        if message.get("type").is_none() && message.get("role").and_then(Value::as_str).is_some() {
            let existing = std::mem::take(message);
            message.insert("type".to_string(), Value::String("message".to_string()));
            message.extend(existing);
        }
        if matches!(
            message.get("type").and_then(Value::as_str),
            None | Some("message")
        ) && let Some(text) = message.get("content").and_then(Value::as_str)
        {
            let content_type = if message.get("role").and_then(Value::as_str) == Some("assistant") {
                "output_text"
            } else {
                "input_text"
            };
            message.insert(
                "content".to_string(),
                serde_json::json!([{"type": content_type, "text": text}]),
            );
        }
    }
    let synthesized_item_ids = responses_lite::assign_missing_item_ids(&mut items);
    *input = Value::Array(items);
    synthesized_item_ids
}

// The fixed Subscription target rejects these public/legacy output-cap, sampling, and stream
// delivery controls. There is no evidence-backed equivalent, so only that profile drops them.
fn strip_unsupported_subscription_fields(object: &mut Map<String, Value>) {
    for field in UNSUPPORTED_SUBSCRIPTION_FIELDS {
        object.remove(*field);
    }
}

// The Subscription backend rejects the public Responses `system` role. Keep this exception at the
// typed message layer so API-key profiles and opaque payload values remain untouched.
fn rewrite_subscription_system_roles(object: &mut Map<String, Value>) {
    let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    for item in items {
        let Some(message) = item.as_object_mut() else {
            continue;
        };
        if matches!(
            message.get("type").and_then(Value::as_str),
            None | Some("message")
        ) && message.get("role").and_then(Value::as_str) == Some("system")
        {
            message.insert("role".to_string(), Value::String("developer".to_string()));
        }
    }
}

fn lite_incremental(object: &Map<String, Value>, transport: EmulationTransport) -> bool {
    transport == EmulationTransport::WebSocket
        && object.get("tools").is_none()
        && object.get("instructions").is_none()
        && object.contains_key("previous_response_id")
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
        "role": "user",
        "content": [{"type": "input_text", "text": text}],
    })
}

fn canonicalize_request_order(object: &mut Map<String, Value>, transport: EmulationTransport) {
    const HTTP_ORDER: &[&str] = &[
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
        "client_metadata",
    ];
    const WEBSOCKET_ORDER: &[&str] = &[
        "type",
        "model",
        "instructions",
        "previous_response_id",
        "stream_id",
        "input",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "reasoning",
        "store",
        "stream_options",
        "include",
        "service_tier",
        "prompt_cache_key",
        "text",
        "generate",
        "client_metadata",
    ];
    let order = if transport == EmulationTransport::WebSocket {
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
    object.extend(existing);
}
