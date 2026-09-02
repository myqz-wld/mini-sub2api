use crate::ascii_json::to_ascii_json_string;
use crate::lifecycle_carriers::turn_metadata_rules;
use serde_json::Map;
use serde_json::Value;

pub(crate) fn bounded_turn_metadata(raw: &str) -> Option<String> {
    let mut value = serde_json::from_str::<Value>(raw).ok()?;
    value.as_object_mut()?.retain(|name, _| {
        turn_metadata_rules().any(|rule| rule.name == name && rule.header_visible())
    });
    to_ascii_json_string(&value).ok()
}

pub(super) fn complete_turn_metadata(raw: &str, generated: &str) -> Option<String> {
    let mut existing = serde_json::from_str::<Value>(raw).ok()?;
    let existing = existing.as_object_mut()?;
    let before = existing.len();
    existing.retain(|name, _| turn_metadata_rules().any(|rule| rule.name == name));
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
    to_ascii_json_string(&Value::Object(std::mem::take(existing))).ok()
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
