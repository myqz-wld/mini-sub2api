use serde_json::Map;
use serde_json::Value;

#[derive(Clone, Copy)]
pub(crate) struct ModelProfile {
    pub(crate) responses_lite: bool,
    reasoning_effort: Option<&'static str>,
    reasoning_summary: Option<&'static str>,
    verbosity: Option<&'static str>,
}

pub(crate) fn model_profile(model: &str) -> ModelProfile {
    match model {
        "gpt-5.6-sol" => profile(true, Some("low"), None, Some("low")),
        "gpt-5.6-terra" | "gpt-5.6-luna" => profile(true, Some("medium"), None, Some("low")),
        "gpt-5.4-mini" => profile(false, Some("medium"), None, Some("medium")),
        "gpt-5.2" => profile(false, Some("medium"), Some("auto"), Some("low")),
        "gpt-5.5" | "gpt-5.4" | "codex-auto-review" => {
            profile(false, Some("medium"), None, Some("low"))
        }
        _ => profile(false, None, Some("auto"), None),
    }
}

const fn profile(
    responses_lite: bool,
    reasoning_effort: Option<&'static str>,
    reasoning_summary: Option<&'static str>,
    verbosity: Option<&'static str>,
) -> ModelProfile {
    ModelProfile {
        responses_lite,
        reasoning_effort,
        reasoning_summary,
        verbosity,
    }
}

pub(crate) fn normalize_optional_members(object: &mut Map<String, Value>) -> bool {
    let mut removed = false;
    for name in [
        "instructions",
        "tools",
        "stream_options",
        "service_tier",
        "prompt_cache_key",
        "previous_response_id",
        "generate",
        "text",
    ] {
        if object.get(name).is_some_and(Value::is_null) {
            object.remove(name);
            removed = true;
        }
    }
    removed
}

pub(crate) fn merge_request_defaults(object: &mut Map<String, Value>, profile: ModelProfile) {
    object.insert("store".to_string(), Value::Bool(false));
    object.insert("stream".to_string(), Value::Bool(true));
    if object.get("tool_choice").and_then(Value::as_str).is_none() {
        object.insert("tool_choice".to_string(), Value::String("auto".to_string()));
    }
    if profile.responses_lite {
        object.insert("parallel_tool_calls".to_string(), Value::Bool(false));
    } else if !object
        .get("parallel_tool_calls")
        .is_some_and(Value::is_boolean)
    {
        object.insert("parallel_tool_calls".to_string(), Value::Bool(true));
    }
    merge_reasoning(object, profile);
    merge_text(object, profile);
    merge_include(object);
}

fn merge_reasoning(object: &mut Map<String, Value>, profile: ModelProfile) {
    let reasoning = object
        .entry("reasoning".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !reasoning.is_object() {
        *reasoning = Value::Object(Map::new());
    }
    let reasoning = reasoning.as_object_mut().expect("reasoning object");
    remove_null_members(reasoning, &["effort", "summary", "context"]);
    if let Some(effort) = profile.reasoning_effort {
        reasoning
            .entry("effort".to_string())
            .or_insert_with(|| Value::String(effort.to_string()));
    }
    if reasoning.get("summary").and_then(Value::as_str) == Some("none") {
        reasoning.remove("summary");
    }
    if let Some(summary) = profile.reasoning_summary {
        reasoning
            .entry("summary".to_string())
            .or_insert_with(|| Value::String(summary.to_string()));
    }
    if profile.responses_lite {
        reasoning
            .entry("context".to_string())
            .or_insert_with(|| Value::String("all_turns".to_string()));
    }
}

fn merge_text(object: &mut Map<String, Value>, profile: ModelProfile) {
    if object.get("text").is_none() && profile.verbosity.is_none() {
        return;
    }
    let text = object
        .entry("text".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !text.is_object() {
        *text = Value::Object(Map::new());
    }
    let text = text.as_object_mut().expect("text object");
    remove_null_members(text, &["verbosity", "format"]);
    if let Some(verbosity) = profile.verbosity {
        text.entry("verbosity".to_string())
            .or_insert_with(|| Value::String(verbosity.to_string()));
    }
    if text.is_empty() {
        object.remove("text");
    }
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
        Some(_) | None => {
            object.insert(
                "include".to_string(),
                serde_json::json!(["reasoning.encrypted_content"]),
            );
        }
    }
}

fn remove_null_members(object: &mut Map<String, Value>, names: &[&str]) {
    for name in names {
        if object.get(*name).is_some_and(Value::is_null) {
            object.remove(*name);
        }
    }
}
