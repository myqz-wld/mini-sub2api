use crate::ascii_json::to_ascii_json_string;
use crate::lifecycle_carriers::turn_metadata_rules;
use serde_json::Map;
use serde_json::Value;

pub(crate) fn bounded_turn_metadata(raw: &str) -> Option<String> {
    let mut value = serde_json::from_str::<Value>(raw).ok()?;
    value.as_object_mut()?.retain(|name, value| {
        turn_metadata_rules().any(|rule| rule.name == name && rule.header_visible())
            || is_extra_metadata(name, value)
    });
    to_ascii_json_string(&value).ok()
}

pub(super) fn complete_turn_metadata(raw: &str, generated: &str) -> Option<String> {
    let mut existing = serde_json::from_str::<Value>(raw).ok()?;
    let existing = existing.as_object_mut()?;
    let before = existing.len();
    existing.retain(|name, value| {
        turn_metadata_rules().any(|rule| rule.name == name) || is_extra_metadata(name, value)
    });
    let stripped = existing.len() != before;
    if existing.get("request_kind").and_then(Value::as_str) == Some("memory") {
        return if stripped {
            encode_reordered(existing, None)
        } else {
            Some(raw.to_string())
        };
    }
    // Codex 0.156.0 deliberately emits startup prewarm metadata with an empty turn ID and without
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
    let complete = turn_metadata_rules()
        .filter(|rule| rule.normal_required())
        .all(|rule| existing.contains_key(rule.name));
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
    for rule in turn_metadata_rules() {
        if let Some(value) = remainder
            .remove(rule.name)
            .or_else(|| generated.and_then(|generated| generated.get(rule.name).cloned()))
        {
            existing.insert(rule.name.to_string(), value);
        }
    }
    existing.extend(remainder);
    to_ascii_json_string(&Value::Object(std::mem::take(existing))).ok()
}

// Native app-server extras are strings with reserved keys removed. Its stricter 16/64/128
// configuration limits do not apply to that public turn/start path. These fields use the existing
// request/assembly byte limits, remain opaque, and are never stored in the durable identity ledger.
fn is_extra_metadata(name: &str, value: &Value) -> bool {
    value.is_string()
        && !turn_metadata_rules().any(|rule| rule.name == name)
        && ![
            "x-codex-installation-id",
            "x-codex-window-id",
            "x-codex-turn-metadata",
            "x-codex-parent-thread-id",
            "x-openai-subagent",
            "code_mode_tool_names",
        ]
        .contains(&name)
}

fn is_complete_native_prewarm_metadata(metadata: &Map<String, Value>) -> bool {
    metadata.get("request_kind").and_then(Value::as_str) == Some("prewarm")
        && metadata.get("turn_id").and_then(Value::as_str) == Some("")
        && !metadata.contains_key("root_turn_id")
        && !metadata.contains_key("turn_started_at_unix_ms")
        && turn_metadata_rules()
            .filter(|rule| rule.prewarm_required_string())
            .all(|rule| {
                metadata
                    .get(rule.name)
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty())
            })
        && turn_metadata_rules()
            .filter(|rule| rule.prewarm_required_bool())
            .all(|rule| metadata.get(rule.name).is_some_and(Value::is_boolean))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn app_server_string_extras_survive_completion_and_ascii_header_encoding() {
        let mut metadata = json!({"request_kind":"memory","nonstring":{"ignored":true},
            "tool_namespaces_info":{"body_only":true}, "x-codex-parent-thread-id":"reserved",
            "code_mode_tool_names":"reserved", "x-codex-installation-id":"reserved"});
        for index in 0..20 {
            metadata[format!("extra_{index}")] = Value::String("é🚀".repeat(40));
        }
        metadata["任意 key"] = "native app-server string".into();
        let completed = complete_turn_metadata(&metadata.to_string(), "{}").unwrap();
        let header = bounded_turn_metadata(&completed).unwrap();
        assert!(header.is_ascii());
        for raw in [&completed, &header] {
            let parsed: Value = serde_json::from_str(raw).unwrap();
            for index in 0..20 {
                assert_eq!(
                    parsed[format!("extra_{index}")],
                    metadata[format!("extra_{index}")]
                );
            }
            assert_eq!(parsed["任意 key"], metadata["任意 key"]);
            for name in [
                "nonstring",
                "x-codex-parent-thread-id",
                "code_mode_tool_names",
                "x-codex-installation-id",
            ] {
                assert!(parsed.get(name).is_none());
            }
        }
        let body: Value = serde_json::from_str(&completed).unwrap();
        let header: Value = serde_json::from_str(&header).unwrap();
        assert!(body.get("tool_namespaces_info").is_some());
        assert!(header.get("tool_namespaces_info").is_none());
    }
}
