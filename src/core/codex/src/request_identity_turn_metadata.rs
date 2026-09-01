use crate::ascii_json::to_ascii_json_string;
use serde_json::Map;
use serde_json::Value;

const TURN_METADATA_FIELDS: &[&str] = &[
    "installation_id",
    "session_id",
    "thread_id",
    "agent_name",
    "turn_id",
    "window_id",
    "request_kind",
    "forked_from_thread_id",
    "parent_thread_id",
    "parent_turn_id",
    "root_turn_id",
    "subagent_kind",
    "thread_source",
    "sandbox",
    "sandbox_mode",
    "auto_review_enabled",
    "node_repl_auto_review_required",
    "node_repl_disabled",
    "workspaces",
    "tool_namespaces_info",
    "turn_started_at_unix_ms",
    "compaction",
];

pub(crate) fn bounded_turn_metadata(raw: &str) -> Option<String> {
    let mut value = serde_json::from_str::<Value>(raw).ok()?;
    value.as_object_mut()?.retain(|name, _| {
        name != "tool_namespaces_info" && TURN_METADATA_FIELDS.contains(&name.as_str())
    });
    to_ascii_json_string(&value).ok()
}

pub(super) fn complete_turn_metadata(raw: &str, generated: &str) -> Option<String> {
    let mut existing = serde_json::from_str::<Value>(raw).ok()?;
    let existing = existing.as_object_mut()?;
    let before = existing.len();
    existing.retain(|name, _| TURN_METADATA_FIELDS.contains(&name.as_str()));
    let stripped = existing.len() != before;
    if existing.get("request_kind").and_then(Value::as_str) == Some("memory") {
        return if stripped {
            encode_reordered(existing, None)
        } else {
            Some(raw.to_string())
        };
    }
    // Codex 0.149.0 deliberately emits startup prewarm metadata with an empty turn ID and without
    // root-turn or turn-start fields. That native shape is complete and must remain byte-stable.
    if is_complete_native_prewarm_metadata(existing) {
        return if stripped {
            encode_reordered(existing, None)
        } else {
            Some(raw.to_string())
        };
    }
    let generated = serde_json::from_str::<Value>(generated).ok()?;
    let generated = generated.as_object()?;
    let complete = [
        "installation_id",
        "session_id",
        "thread_id",
        "agent_name",
        "window_id",
        "request_kind",
        "auto_review_enabled",
        "node_repl_auto_review_required",
        "node_repl_disabled",
        "turn_started_at_unix_ms",
    ]
    .iter()
    .all(|name| existing.contains_key(*name));
    if complete && !stripped {
        return Some(raw.to_string());
    }
    encode_reordered(existing, Some(generated))
}

fn encode_reordered(
    existing: &mut Map<String, Value>,
    generated: Option<&Map<String, Value>>,
) -> Option<String> {
    let mut remainder = std::mem::take(existing);
    for name in TURN_METADATA_FIELDS {
        if let Some(value) = remainder
            .remove(*name)
            .or_else(|| generated.and_then(|generated| generated.get(*name).cloned()))
        {
            existing.insert((*name).to_string(), value);
        }
    }
    to_ascii_json_string(&Value::Object(std::mem::take(existing))).ok()
}

fn is_complete_native_prewarm_metadata(metadata: &Map<String, Value>) -> bool {
    metadata.get("request_kind").and_then(Value::as_str) == Some("prewarm")
        && metadata.get("turn_id").and_then(Value::as_str) == Some("")
        && !metadata.contains_key("root_turn_id")
        && !metadata.contains_key("turn_started_at_unix_ms")
        && [
            "installation_id",
            "session_id",
            "thread_id",
            "agent_name",
            "window_id",
            "sandbox",
            "sandbox_mode",
        ]
        .iter()
        .all(|name| {
            metadata
                .get(*name)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        })
        && [
            "auto_review_enabled",
            "node_repl_auto_review_required",
            "node_repl_disabled",
        ]
        .iter()
        .all(|name| metadata.get(*name).is_some_and(Value::is_boolean))
}
