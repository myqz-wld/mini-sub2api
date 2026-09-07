use serde_json::{Map, Value};

#[cfg(test)]
#[path = "codex_instruction_snapshots.rs"]
mod snapshots;
#[cfg(test)]
pub(crate) use snapshots::for_model;

/// Omits an absent or invalid caller base without introducing model prompt text.
pub(crate) fn normalize_base(object: &mut Map<String, Value>) {
    if !has_valid_instructions(object) {
        object.remove("instructions");
    }
}

/// Keeps caller base text verbatim, placing it after the tool prefix only for Lite.
pub(crate) fn apply(object: &mut Map<String, Value>, responses_lite: bool) -> Result<(), ()> {
    if !responses_lite && object.get("input").is_some_and(|input| !input.is_array()) {
        return Err(());
    }
    normalize_base(object);
    if !responses_lite {
        return Ok(());
    }
    let base = object.remove("instructions");
    let input = input_items(object)?;
    if let Some(Value::String(base)) = base {
        let insertion = usize::from(
            input
                .first()
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
                == Some("additional_tools"),
        );
        input.insert(insertion, developer_message(base));
    }
    Ok(())
}

pub(crate) fn has_valid_instructions(object: &Map<String, Value>) -> bool {
    object
        .get("instructions")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty())
}

fn input_items(object: &mut Map<String, Value>) -> Result<&mut Vec<Value>, ()> {
    object
        .entry("input".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or(())
}

fn developer_message(text: String) -> Value {
    serde_json::json!({
        "type": "message",
        "role": "developer",
        "content": [{"type": "input_text", "text": text}],
    })
}
