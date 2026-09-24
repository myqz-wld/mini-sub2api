//! Guardian v2 has its own producer metadata and transport, independent of ModelClient.
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_state_editor::RequestStateEditor;
use crate::request_state_types::WireIdDomain;
use anyhow::Result;
use http::HeaderMap;
use serde_json::{Map, Value};

pub(crate) const TURN: &str = "x-codex-turn-metadata";
pub(crate) const SOURCE: &str = "guardian_classifier_source_thread_id";
pub(crate) const LITE: &str = "ws_request_header_x_openai_internal_codex_responses_lite";

pub(crate) fn selected(headers: &HeaderMap) -> bool {
    headers
        .get("x-codex-guardian")
        .is_some_and(|v| v == "classifier")
}

pub(crate) fn source(object: &Map<String, Value>) -> Option<String> {
    let metadata = object.get("client_metadata")?;
    let turn: Value = serde_json::from_str(metadata.get(TURN)?.as_str()?).ok()?;
    turn.get(SOURCE)?.as_str().map(str::to_owned)
}

pub(crate) fn overlay(object: &mut Map<String, Value>, headers: &mut HeaderMap) {
    if let Some(metadata) = object
        .get_mut("client_metadata")
        .and_then(Value::as_object_mut)
    {
        crate::ignored_fields::retain(
            metadata,
            &[
                "session_id",
                "thread_id",
                "turn_id",
                "parent_turn_id",
                "root_turn_id",
                "parent_response_id",
                "x-openai-subagent",
                "x-codex-window-id",
                TURN,
                LITE,
            ],
            "client_metadata",
        );
        if let Some(raw) = metadata.get(TURN).and_then(Value::as_str)
            && let Ok(mut turn) = serde_json::from_str::<Map<String, Value>>(raw)
        {
            crate::ignored_fields::retain(
                &mut turn,
                &[
                    "session_id",
                    "thread_id",
                    "turn_id",
                    "parent_turn_id",
                    "root_turn_id",
                    SOURCE,
                    "thread_source",
                    "turn_trigger",
                ],
                "turn_metadata",
            );
            metadata.insert(
                TURN.into(),
                serde_json::to_string(&turn).expect("JSON metadata").into(),
            );
        }
    }
    headers.insert(
        "x-openai-internal-codex-responses-lite",
        "true".parse().unwrap(),
    );
    headers.insert("x-openai-subagent", "guardian".parse().unwrap());
    crate::request_identity::remove_routing_hint(headers);
}

pub(crate) fn normalize_model(object: &mut Map<String, Value>) {
    if object.get("model").and_then(Value::as_str) != Some("gpt-5.6-luna") {
        crate::ignored_fields::record("request", "model", "role_policy");
        object.insert("model".into(), "gpt-5.6-luna".into());
    }
    crate::ignored_fields::remove(object, "tools", "request", "role_policy");
    if let Some(first) = object
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .and_then(|items| items.first_mut())
        && first.get("type").and_then(Value::as_str) == Some("additional_tools")
        && first
            .get("tools")
            .is_some_and(|v| v.as_array().is_none_or(|tools| !tools.is_empty()))
    {
        crate::ignored_fields::record("input[]", "tools", "role_policy");
        first["tools"] = serde_json::json!([]);
    }
}

pub(crate) fn project(
    editor: &mut RequestStateEditor<'_>,
    object: &mut Map<String, Value>,
    headers: &mut HeaderMap,
    identity: &ResolvedRequestIdentity,
    raw_source: Option<&str>,
    has_root: bool,
) -> Result<()> {
    // Any supplied continuation reference has already passed scoped translation and expansion.
    object.shift_remove("previous_response_id");
    let source = match raw_source {
        Some(raw) => editor
            .existing_wire_from_downstream(WireIdDomain::Thread, raw)?
            .or(editor.existing_wire_from_downstream(WireIdDomain::Session, raw)?)
            .ok_or_else(|| anyhow::anyhow!("classifier source unavailable"))?,
        None => identity
            .parent_thread_id
            .clone()
            .unwrap_or_else(|| identity.session_id.clone()),
    };
    anyhow::ensure!(
        identity.parent_thread_id.as_deref() == Some(source.as_str()),
        "classifier source relationship changed"
    );
    if let Some(parent) = identity.parent_turn_id.as_deref() {
        let (_, turn) = editor
            .turn_by_id(parent)
            .ok_or_else(|| anyhow::anyhow!("classifier parent unavailable"))?;
        anyhow::ensure!(
            turn.thread_id == source,
            "classifier parent belongs to another source"
        );
    }
    object.insert(
        "prompt_cache_key".into(),
        format!("guardian-v2:{source}").into(),
    );
    let metadata = object
        .get_mut("client_metadata")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("classifier metadata missing"))?;
    metadata.retain(|name, _| {
        matches!(
            name.as_str(),
            "session_id"
                | "thread_id"
                | "turn_id"
                | "parent_turn_id"
                | "root_turn_id"
                | "parent_response_id"
                | "x-openai-subagent"
                | "x-codex-window-id"
        ) || name == TURN
            || name == LITE
    });
    metadata.insert("x-openai-subagent".into(), "guardian".into());
    metadata.insert(
        "x-codex-window-id".into(),
        format!("{}:0", identity.thread_id).into(),
    );
    metadata.insert(LITE.into(), "true".into());
    if !has_root {
        metadata.remove("root_turn_id");
    }
    // The released classifier preserves json! insertion order, then appends optional root.
    // ModelClient's struct/BTreeMap ordering does not apply to this producer.
    let mut turn = Map::new();
    for (key, value) in [
        ("session_id", identity.session_id.clone().into()),
        ("thread_id", identity.thread_id.clone().into()),
        (SOURCE, source.into()),
        (
            "turn_id",
            identity.turn_id.clone().unwrap_or_default().into(),
        ),
    ] {
        turn.insert(key.into(), value);
    }
    if let Some(parent) = &identity.parent_turn_id {
        turn.insert("parent_turn_id".into(), parent.clone().into());
    }
    turn.insert("thread_source".into(), "guardian_classifier".into());
    turn.insert("turn_trigger".into(), "guardian_classifier".into());
    if has_root && let Some(root) = &identity.root_turn_id {
        turn.insert("root_turn_id".into(), root.clone().into());
    }
    metadata.insert(TURN.into(), serde_json::to_string(&turn)?.into());
    for name in [
        TURN,
        "x-codex-beta-features",
        "x-codex-parent-thread-id",
        "x-codex-routing-hint",
        "x-codex-installation-id",
        "x-codex-turn-state",
        "x-codex-inference-call-id",
    ] {
        headers.remove(name);
    }
    headers.insert(
        "x-codex-window-id",
        format!("{}:0", identity.thread_id).parse()?,
    );
    headers.insert("x-client-request-id", identity.thread_id.parse()?);
    Ok(())
}
