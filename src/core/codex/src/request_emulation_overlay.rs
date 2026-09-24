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

// Pinned ResponsesApiRequest / ResponseCreateWsRequest fields. Local reference carriers
// are consumed by admission and reconstructed before this final projection.
const SUPPORTED_REQUEST_FIELDS: &[&str] = &[
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
    "access_programs",
];
const SUPPORTED_WEBSOCKET_FIELDS: &[&str] = &["type", "generate", "previous_response_id"];

pub(super) fn apply(
    object: &mut Map<String, Value>,
    headers: &mut HeaderMap,
    transport: EmulationTransport,
    profile: UpstreamProfile,
    force_lite: bool,
) -> Result<(Vec<String>, Vec<usize>), ()> {
    let role = crate::native_request_policy::Role::read(object, headers);
    let model = request_defaults::diagnostic_model(
        object
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    crate::ignored_fields::scope(transport, role.label(), model, "construction", || {
        apply_inner(object, headers, transport, profile, force_lite, role)
    })
}

fn apply_inner(
    object: &mut Map<String, Value>,
    headers: &mut HeaderMap,
    transport: EmulationTransport,
    profile: UpstreamProfile,
    force_lite: bool,
    role: crate::native_request_policy::Role,
) -> Result<(Vec<String>, Vec<usize>), ()> {
    crate::reasoning_visibility::ReasoningVisibility::read(object).map_err(|_| ())?;
    crate::native_request_policy::filter_admission(object);
    if role == crate::native_request_policy::Role::Classifier {
        crate::request_classifier::normalize_model(object);
    }
    let caller_base = codex_instructions::has_valid_instructions(object);
    retain_codex_fields(object, transport);
    if object.contains_key("conversation") {
        crate::ignored_fields::record("request", "conversation", "local_reference_only");
    }

    let mut model_profile = object
        .get("model")
        .and_then(Value::as_str)
        .map(request_defaults::model_profile)
        .unwrap_or_else(|| request_defaults::model_profile(""));
    model_profile.responses_lite |= force_lite || responses_lite_requested(object);
    let lite_incremental = model_profile.responses_lite && lite_incremental(object, transport);
    let already_lite =
        model_profile.responses_lite && (responses_lite_requested(object) || lite_incremental);

    let synthesized_item_ids =
        if already_lite && role != crate::native_request_policy::Role::Classifier {
            Vec::new()
        } else {
            normalize_input(object)
        };
    codex_instructions::apply(object, model_profile.responses_lite)?;
    if model_profile.responses_lite {
        if !already_lite {
            relocate_lite_tools(object);
        }
    } else {
        canonicalize_top_level_tools(object);
    }
    if profile.uses_subscription_transport() {
        strip_unsupported_subscription_fields(object);
        rewrite_subscription_system_roles(object);
    }

    // Build local metadata before effort aliases are resolved for the wire.
    let selected_effort = object
        .get("reasoning")
        .and_then(|r| r.get("effort"))
        .cloned();
    request_defaults::merge_for_role(
        object,
        model_profile,
        transport == EmulationTransport::Http,
        role,
    );
    let wire_effort = object
        .get("reasoning")
        .and_then(|r| r.get("effort"))
        .cloned();
    if let Some(effort) = &selected_effort
        && let Some(reasoning) = object.get_mut("reasoning").and_then(Value::as_object_mut)
    {
        reasoning.insert("effort".into(), effort.clone());
    }
    enforce_upstream_transport_controls(object, transport);
    let guardian_reviewer = headers
        .get("x-codex-guardian")
        .is_some_and(|value| value == "reviewer");
    if guardian_reviewer {
        object.remove("service_tier");
    }
    if profile.uses_subscription_transport()
        && !guardian_reviewer
        && role != crate::native_request_policy::Role::Classifier
    {
        request_identity::apply_routing_hint(object, headers);
    } else {
        request_identity::remove_routing_hint(headers);
    }
    if role == crate::native_request_policy::Role::Classifier {
        crate::request_classifier::overlay(object, headers);
    } else {
        request_identity::apply(
            object,
            headers,
            IdentityContext {
                responses_lite: model_profile.responses_lite,
                transport,
                tool_namespaces_info: None,
            },
        );
    }
    if let Some(metadata) = object
        .get_mut("client_metadata")
        .and_then(Value::as_object_mut)
    {
        metadata.retain(|_, value| {
            if value.is_string() {
                true
            } else {
                crate::ignored_fields::record("client_metadata", "unknown", "non_string_metadata");
                false
            }
        });
    }
    if let Some(effort) = wire_effort
        && let Some(reasoning) = object.get_mut("reasoning").and_then(Value::as_object_mut)
    {
        reasoning.insert("effort".into(), effort);
    }
    crate::response_item_metadata::normalize_images(object.get_mut("input"), model_profile);
    // Recorded now, removed from the sending copy only after state projection has completed.
    if let Some(input) = object.get("input").and_then(Value::as_array) {
        for item in input {
            if item.get("type").and_then(Value::as_str) == Some("configuration_update") {
                crate::ignored_fields::record(
                    "input[]",
                    "configuration_update",
                    "feature_disabled",
                );
            }
            if item.get("type").and_then(Value::as_str) == Some("item_reference") {
                crate::ignored_fields::record("input[]", "item_reference", "unsupported_variant");
            }
        }
    }
    responses_lite::canonicalize_request_items_for_role(
        object,
        (!model_profile.responses_lite).then_some("high"),
        role,
    );
    canonicalize_request_order(object, transport);
    let mut prefixes = Vec::new();
    if model_profile.responses_lite
        && let Some(input) = object.get("input").and_then(Value::as_array)
        && input.first().is_some_and(|item| {
            item.get("type").and_then(Value::as_str) == Some("additional_tools")
        })
    {
        if input[0].get("id").is_none() {
            prefixes.push(0);
        }
        if caller_base {
            prefixes.push(1);
        }
    }
    Ok((synthesized_item_ids, prefixes))
}

fn retain_codex_fields(object: &mut Map<String, Value>, transport: EmulationTransport) {
    let mut fields = SUPPORTED_REQUEST_FIELDS.to_vec();
    fields.extend(["conversation", "previous_response_id"]);
    if transport == EmulationTransport::WebSocket {
        fields.extend(SUPPORTED_WEBSOCKET_FIELDS);
    }
    crate::ignored_fields::retain(object, &fields, "request");
}

/// History eligibility compares effective caller settings using the same field policy as sending.
/// This does not rewrite messages, identities, tools or instruction placement.
pub(crate) fn filter_subscription_fields(
    object: &mut Map<String, Value>,
    transport: EmulationTransport,
) {
    retain_codex_fields(object, transport);
    strip_unsupported_subscription_fields(object);
}

fn enforce_upstream_transport_controls(
    object: &mut Map<String, Value>,
    transport: EmulationTransport,
) {
    object.insert("store".to_string(), Value::Bool(false));
    let _ = transport;
    object.insert("stream".to_string(), Value::Bool(true));
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
    // Native ordinary Responses serializes Some(tools), including an empty list. Lite
    // omits this carrier and puts tools in its setup item instead. Explicit null stays caller-owned.
    object
        .entry("tools")
        .or_insert_with(|| Value::Array(Vec::new()));
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
    let summary_delivery = object
        .get("stream_options")
        .and_then(Value::as_object)
        .and_then(|options| options.get("reasoning_summary_delivery"))
        .filter(|value| value.as_str() == Some("sequential_cutoff"))
        .cloned();
    if let Some(options) = object
        .get_mut("stream_options")
        .and_then(Value::as_object_mut)
    {
        crate::ignored_fields::retain(options, &["reasoning_summary_delivery"], "stream_options");
    }
    if summary_delivery.is_none() {
        crate::ignored_fields::remove(object, "stream_options", "request", "unsupported_field");
    }
    if let Some(delivery) = summary_delivery {
        object.insert(
            "stream_options".into(),
            serde_json::json!({"reasoning_summary_delivery":delivery}),
        );
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
        && !codex_instructions::has_valid_instructions(object)
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

pub(super) fn canonicalize_request_order(
    object: &mut Map<String, Value>,
    transport: EmulationTransport,
) {
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
        "access_programs",
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
        "access_programs",
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
