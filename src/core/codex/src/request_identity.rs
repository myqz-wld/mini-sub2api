use crate::ascii_json::to_ascii_json_string;
use chrono::Utc;
use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use serde_json::Map;
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

#[path = "request_identity_prewarm.rs"]
mod prewarm;
#[path = "request_identity_turn_metadata.rs"]
pub(crate) mod turn_metadata;

use turn_metadata::bounded_turn_metadata;
use turn_metadata::complete_turn_metadata;

const INSTALLATION_HEADER: &str = "x-codex-installation-id";
const TURN_METADATA_HEADER: &str = "x-codex-turn-metadata";
const WINDOW_HEADER: &str = "x-codex-window-id";
const ROUTING_HINT_HEADER: &str = "x-codex-routing-hint";
const BETA_FEATURES_HEADER: &str = "x-codex-beta-features";
const RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";
const WS_RESPONSES_LITE_METADATA: &str = "ws_request_header_x_openai_internal_codex_responses_lite";
const WS_STREAM_START_METADATA: &str = "x-codex-ws-stream-request-start-ms";
const DEFAULT_BETA_FEATURE: &str = "remote_compaction_v2";

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum SubscriptionTransport {
    Http,
    WebSocket,
}

pub(crate) struct IdentityContext {
    pub(crate) responses_lite: bool,
    pub(crate) transport: SubscriptionTransport,
    pub(crate) tool_namespaces_info: Option<Value>,
}

struct RequestIdentity {
    session_id: String,
    thread_id: String,
    installation_id: String,
    turn_id: Option<String>,
    root_turn_id: Option<String>,
    window_id: String,
    body_turn_metadata: String,
    header_turn_metadata: String,
}

pub(crate) fn apply(
    object: &mut Map<String, Value>,
    headers: &mut HeaderMap,
    context: IdentityContext,
) {
    let identity = resolve_identity(object, headers, &context);
    apply_headers(headers, &identity, &context);
    apply_client_metadata(object, headers, &identity, &context);
    if !object.contains_key("prompt_cache_key") {
        object.insert(
            "prompt_cache_key".to_string(),
            Value::String(identity.session_id),
        );
    }
}

pub(crate) fn apply_synthetic_prewarm(
    object: &mut Map<String, Value>,
    headers: &mut HeaderMap,
) -> Result<(), ()> {
    prewarm::apply(object, headers)
}

pub(crate) fn apply_routing_hint(object: &Map<String, Value>, headers: &mut HeaderMap) {
    headers.remove(ROUTING_HINT_HEADER);
    let Some(model) = object.get("model").and_then(Value::as_str) else {
        return;
    };
    if model.is_empty() {
        return;
    }
    let hint = object
        .get("service_tier")
        .and_then(Value::as_str)
        .map_or_else(
            || format!("model={model}"),
            |tier| format!("model={model};tier={tier}"),
        );
    if let Ok(value) = HeaderValue::from_str(&hint) {
        headers.insert(HeaderName::from_static(ROUTING_HINT_HEADER), value);
    }
}

pub(crate) fn remove_routing_hint(headers: &mut HeaderMap) {
    headers.remove(ROUTING_HINT_HEADER);
}

fn resolve_identity(
    object: &Map<String, Value>,
    headers: &HeaderMap,
    context: &IdentityContext,
) -> RequestIdentity {
    let session_id = header_text(headers, "session-id")
        .or_else(|| client_metadata_text(object, "session_id"))
        .unwrap_or_else(new_uuid_v7);
    let thread_id = header_text(headers, "thread-id")
        .or_else(|| client_metadata_text(object, "thread_id"))
        .unwrap_or_else(|| session_id.clone());
    let installation_id = client_metadata_text(object, INSTALLATION_HEADER)
        .or_else(|| header_text(headers, INSTALLATION_HEADER))
        .unwrap_or_else(new_uuid_v7);
    let body_turn_metadata = client_metadata_text(object, TURN_METADATA_HEADER);
    let header_turn_metadata = header_text(headers, TURN_METADATA_HEADER);
    let request_header_turn_metadata = (context.transport == SubscriptionTransport::Http)
        .then_some(header_turn_metadata.as_deref())
        .flatten();
    let prewarm = context.transport == SubscriptionTransport::WebSocket
        && object.get("generate").and_then(Value::as_bool) == Some(false);
    let request_kind = body_turn_metadata
        .as_deref()
        .and_then(|raw| metadata_text(raw, "request_kind"))
        .or_else(|| request_header_turn_metadata.and_then(|raw| metadata_text(raw, "request_kind")))
        .unwrap_or_else(|| if prewarm { "prewarm" } else { "turn" }.to_string());
    let memory = request_kind == "memory";
    let turn_id = (!memory).then(|| {
        body_turn_metadata
            .as_deref()
            .and_then(|raw| metadata_text(raw, "turn_id"))
            .or_else(|| request_header_turn_metadata.and_then(|raw| metadata_text(raw, "turn_id")))
            .or_else(|| client_metadata_text(object, "turn_id"))
            .unwrap_or_else(new_uuid_v7)
    });
    let root_turn_id = body_turn_metadata
        .as_deref()
        .and_then(|raw| metadata_text(raw, "root_turn_id"))
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            client_metadata_text(object, "root_turn_id").filter(|value| !value.trim().is_empty())
        })
        .or_else(|| turn_id.clone().filter(|value| !value.trim().is_empty()));
    let window_id = header_text(headers, WINDOW_HEADER)
        .or_else(|| client_metadata_text(object, WINDOW_HEADER))
        .unwrap_or_else(|| format!("{thread_id}:0"));
    let generated = generated_turn_metadata(
        headers,
        &installation_id,
        &session_id,
        &thread_id,
        turn_id.as_deref(),
        root_turn_id.as_deref(),
        &window_id,
        &request_kind,
        context.tool_namespaces_info.as_ref(),
    );
    let body_turn_metadata = body_turn_metadata
        .and_then(|raw| complete_turn_metadata(&raw, &generated))
        .unwrap_or_else(|| generated.clone());
    let body_turn_metadata =
        crate::sandbox_projection::normalize_serialized(&body_turn_metadata, &request_kind)
            .unwrap_or(body_turn_metadata);
    let bounded_generated = bounded_turn_metadata(&generated).unwrap_or_else(|| generated.clone());
    let header_turn_metadata = if context.transport == SubscriptionTransport::WebSocket {
        bounded_turn_metadata(&body_turn_metadata).unwrap_or(bounded_generated)
    } else {
        header_turn_metadata
            .and_then(|raw| complete_turn_metadata(&raw, &bounded_generated))
            .or_else(|| bounded_turn_metadata(&body_turn_metadata))
            .unwrap_or(bounded_generated)
    };
    let header_turn_metadata =
        crate::sandbox_projection::normalize_serialized(&header_turn_metadata, &request_kind)
            .unwrap_or(header_turn_metadata);
    RequestIdentity {
        session_id,
        thread_id,
        installation_id,
        turn_id,
        root_turn_id,
        window_id,
        body_turn_metadata,
        header_turn_metadata,
    }
}

#[allow(clippy::too_many_arguments)]
fn generated_turn_metadata(
    headers: &HeaderMap,
    installation_id: &str,
    session_id: &str,
    thread_id: &str,
    turn_id: Option<&str>,
    root_turn_id: Option<&str>,
    window_id: &str,
    request_kind: &str,
    tool_namespaces_info: Option<&Value>,
) -> String {
    let mut metadata = Map::new();
    metadata.insert("installation_id".to_string(), installation_id.into());
    metadata.insert("session_id".to_string(), session_id.into());
    metadata.insert("thread_id".to_string(), thread_id.into());
    metadata.insert("agent_name".to_string(), "/root".into());
    if let Some(turn_id) = turn_id {
        metadata.insert("turn_id".to_string(), turn_id.into());
    }
    metadata.insert("window_id".to_string(), window_id.into());
    metadata.insert("request_kind".to_string(), request_kind.into());
    if let Some(parent_thread_id) = header_text(headers, "x-codex-parent-thread-id") {
        metadata.insert("parent_thread_id".to_string(), parent_thread_id.into());
    }
    if let Some(root_turn_id) = root_turn_id {
        metadata.insert("root_turn_id".to_string(), root_turn_id.into());
    }
    if let Some(subagent_kind) = subagent_kind(headers) {
        metadata.insert("subagent_kind".to_string(), subagent_kind.into());
    }
    metadata.insert("auto_review_enabled".to_string(), false.into());
    metadata.insert("node_repl_auto_review_required".to_string(), false.into());
    metadata.insert("node_repl_disabled".to_string(), false.into());
    if let Some(tool_namespaces_info) = tool_namespaces_info {
        metadata.insert(
            "tool_namespaces_info".to_string(),
            tool_namespaces_info.clone(),
        );
    }
    metadata.insert(
        "turn_started_at_unix_ms".to_string(),
        Utc::now().timestamp_millis().into(),
    );
    to_ascii_json_string(&Value::Object(metadata)).unwrap_or_else(|_| "{}".to_string())
}

fn apply_headers(headers: &mut HeaderMap, identity: &RequestIdentity, context: &IdentityContext) {
    insert_header(headers, "session-id", &identity.session_id);
    insert_header(headers, "thread-id", &identity.thread_id);
    if header_text(headers, "x-client-request-id").is_none_or(|value| value.is_empty()) {
        insert_header(headers, "x-client-request-id", &identity.thread_id);
    }
    insert_header(
        headers,
        TURN_METADATA_HEADER,
        &identity.header_turn_metadata,
    );
    insert_header(headers, WINDOW_HEADER, &identity.window_id);
    headers.remove(INSTALLATION_HEADER);
    ensure_beta_features(headers);
    if context.transport == SubscriptionTransport::Http && context.responses_lite {
        headers.insert(
            HeaderName::from_static(RESPONSES_LITE_HEADER),
            HeaderValue::from_static("true"),
        );
    } else {
        headers.remove(RESPONSES_LITE_HEADER);
    }
}

fn apply_client_metadata(
    object: &mut Map<String, Value>,
    headers: &HeaderMap,
    identity: &RequestIdentity,
    context: &IdentityContext,
) {
    let preserve_native_order = object
        .get("client_metadata")
        .is_some_and(has_complete_native_client_metadata);
    let metadata = object
        .entry("client_metadata".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !metadata.is_object() {
        *metadata = Value::Object(Map::new());
    }
    let metadata = metadata.as_object_mut().expect("client metadata object");
    for (name, value) in [
        ("session_id", Some(identity.session_id.as_str())),
        ("thread_id", Some(identity.thread_id.as_str())),
        ("turn_id", identity.turn_id.as_deref()),
        (INSTALLATION_HEADER, Some(identity.installation_id.as_str())),
        (WINDOW_HEADER, Some(identity.window_id.as_str())),
        ("root_turn_id", identity.root_turn_id.as_deref()),
    ] {
        if let Some(value) = value {
            insert_string_if_invalid(metadata, name, value);
        }
    }
    metadata.insert(
        TURN_METADATA_HEADER.to_string(),
        Value::String(identity.body_turn_metadata.clone()),
    );
    for name in ["x-openai-subagent", "x-codex-parent-thread-id"] {
        if let Some(value) = header_text(headers, name) {
            insert_string_if_invalid(metadata, name, &value);
        }
    }
    if context.transport == SubscriptionTransport::WebSocket {
        if let Some(value) = header_text(headers, "x-codex-turn-state") {
            insert_string_if_invalid(metadata, "x-codex-turn-state", &value);
        }
        ensure_ws_stream_start(metadata);
        if context.responses_lite {
            insert_string_if_invalid(metadata, WS_RESPONSES_LITE_METADATA, "true");
        }
    }
    if !preserve_native_order {
        randomize_synthesized_client_metadata(metadata);
    }
}

fn insert_string_if_invalid(metadata: &mut Map<String, Value>, name: &str, value: &str) {
    if metadata
        .get(name)
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        metadata.insert(name.to_string(), Value::String(value.to_string()));
    }
}

fn ensure_ws_stream_start(metadata: &mut Map<String, Value>) {
    if metadata
        .get(WS_STREAM_START_METADATA)
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        metadata.insert(
            WS_STREAM_START_METADATA.to_string(),
            Value::String(Utc::now().timestamp_millis().to_string()),
        );
    }
}

fn has_complete_native_client_metadata(value: &Value) -> bool {
    let Some(metadata) = value.as_object() else {
        return false;
    };
    [
        INSTALLATION_HEADER,
        "session_id",
        "thread_id",
        WINDOW_HEADER,
        TURN_METADATA_HEADER,
    ]
    .iter()
    .all(|name| metadata.get(*name).and_then(Value::as_str).is_some())
}

fn randomize_synthesized_client_metadata(metadata: &mut Map<String, Value>) {
    let mut source = std::mem::take(metadata);
    let mut randomized = HashMap::new();
    for name in [
        INSTALLATION_HEADER,
        "session_id",
        "thread_id",
        WINDOW_HEADER,
        "turn_id",
        "x-openai-subagent",
        "x-codex-parent-thread-id",
        "parent_turn_id",
        "root_turn_id",
        TURN_METADATA_HEADER,
        WS_RESPONSES_LITE_METADATA,
        "x-codex-turn-state",
        "traceparent",
        "tracestate",
        WS_STREAM_START_METADATA,
    ] {
        if let Some(value) = source.remove(name) {
            randomized.insert(name.to_string(), value);
        }
    }
    randomized.extend(source);
    metadata.extend(randomized);
}

fn ensure_beta_features(headers: &mut HeaderMap) {
    let mut features = header_text(headers, BETA_FEATURES_HEADER)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|feature| !feature.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !features
        .iter()
        .any(|feature| feature == DEFAULT_BETA_FEATURE)
    {
        features.push(DEFAULT_BETA_FEATURE.to_string());
    }
    if let Ok(value) = HeaderValue::from_str(&features.join(",")) {
        headers.insert(HeaderName::from_static(BETA_FEATURES_HEADER), value);
    }
}

fn subagent_kind(headers: &HeaderMap) -> Option<String> {
    let header = header_text(headers, "x-openai-subagent")?;
    Some(match header.as_str() {
        "collab_spawn" => "thread_spawn".to_string(),
        _ => header,
    })
}

fn metadata_text(raw: &str, name: &str) -> Option<String> {
    serde_json::from_str::<Value>(raw)
        .ok()?
        .get(name)?
        .as_str()
        .map(str::to_string)
}

fn client_metadata_text(object: &Map<String, Value>, name: &str) -> Option<String> {
    object
        .get("client_metadata")?
        .as_object()?
        .get(name)?
        .as_str()
        .map(str::to_string)
}

fn insert_header(headers: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(HeaderName::from_static(name), value);
    }
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn new_uuid_v7() -> String {
    Uuid::now_v7().to_string()
}
