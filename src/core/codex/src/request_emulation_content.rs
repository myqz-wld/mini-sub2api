//! Typed ContentItem/ReasoningItem projections; schema and tool business values are opaque.
use serde_json::Value;

pub(super) fn canonicalize(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let fields: &[&str] = match object.get("type").and_then(Value::as_str) {
        Some("input_text" | "output_text" | "summary_text" | "reasoning_text" | "text") => {
            &["type", "text"]
        }
        Some("input_image") => &["type", "image_url", "file_id", "detail"],
        Some("input_audio") => &["type", "audio_url"],
        Some("encrypted_content") => &["type", "encrypted_content"],
        _ => &["type"],
    };
    crate::ignored_fields::retain(object, fields, "input[].content[]");
    let mut rest = std::mem::take(object);
    for field in fields {
        if let Some(value) = rest.shift_remove(*field) {
            object.insert((*field).into(), value);
        }
    }
}

pub(super) fn filter(values: &mut Vec<Value>, kind: Option<&str>, summary: bool) {
    values.retain_mut(|value| {
        let Some(t) = value.get("type").and_then(Value::as_str) else {
            return true;
        };
        let supported = if summary {
            t == "summary_text"
        } else if kind == Some("reasoning") {
            matches!(t, "reasoning_text" | "text")
        } else if kind == Some("agent_message") {
            matches!(t, "input_text" | "encrypted_content")
        } else if matches!(
            kind,
            Some("function_call_output" | "custom_tool_call_output")
        ) {
            matches!(
                t,
                "input_text" | "input_image" | "input_audio" | "encrypted_content"
            )
        } else {
            matches!(
                t,
                "input_text" | "output_text" | "input_image" | "input_audio"
            )
        };
        if !supported {
            crate::ignored_fields::record("input[].content[]", "type", "unsupported_variant");
        }
        supported
    });
}
