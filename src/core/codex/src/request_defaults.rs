use serde_json::Map;
use serde_json::Value;

#[derive(Clone, Copy)]
pub(crate) struct ModelProfile {
    pub(crate) responses_lite: bool,
    reasoning_effort: Option<&'static str>,
    reasoning_summary: Option<&'static str>,
    verbosity: Option<&'static str>,
}

const MODEL_PROFILES: [(&str, ModelProfile); 8] = [
    ("gpt-5.6-sol", profile(true, Some("low"), None, Some("low"))),
    (
        "gpt-5.6-terra",
        profile(true, Some("medium"), None, Some("low")),
    ),
    (
        "gpt-5.6-luna",
        profile(true, Some("medium"), None, Some("low")),
    ),
    (
        "gpt-5.4-mini",
        profile(false, Some("medium"), None, Some("medium")),
    ),
    (
        "gpt-5.2",
        profile(false, Some("medium"), Some("auto"), Some("low")),
    ),
    ("gpt-5.5", profile(false, Some("medium"), None, Some("low"))),
    ("gpt-5.4", profile(false, Some("medium"), None, Some("low"))),
    (
        "codex-auto-review",
        profile(false, Some("medium"), None, Some("low")),
    ),
];

const FALLBACK_PROFILE: ModelProfile = profile(false, None, Some("auto"), None);

pub(crate) fn model_profile(model: &str) -> ModelProfile {
    find_model_by_longest_prefix(model)
        .or_else(|| find_model_by_namespaced_suffix(model))
        .unwrap_or(FALLBACK_PROFILE)
}

fn find_model_by_longest_prefix(model: &str) -> Option<ModelProfile> {
    MODEL_PROFILES
        .iter()
        .filter(|(slug, _)| model.starts_with(slug))
        .max_by_key(|(slug, _)| slug.len())
        .map(|(_, profile)| *profile)
}

fn find_model_by_namespaced_suffix(model: &str) -> Option<ModelProfile> {
    let (namespace, suffix) = model.split_once('/')?;
    if suffix.contains('/')
        || namespace.is_empty()
        || !namespace
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return None;
    }
    find_model_by_longest_prefix(suffix)
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
    let summary_explicitly_disabled =
        reasoning.get("summary").and_then(Value::as_str) == Some("none");
    remove_null_members(reasoning, &["effort", "summary", "context"]);
    if let Some(effort) = profile.reasoning_effort {
        reasoning
            .entry("effort".to_string())
            .or_insert_with(|| Value::String(effort.to_string()));
    }
    if summary_explicitly_disabled {
        reasoning.remove("summary");
    }
    if !summary_explicitly_disabled && let Some(summary) = profile.reasoning_summary {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_lookup_uses_longest_prefix_then_single_namespace_suffix() {
        let derived_mini = model_profile("gpt-5.4-mini-preview");
        assert_eq!(derived_mini.reasoning_effort, Some("medium"));
        assert_eq!(derived_mini.verbosity, Some("medium"));

        let namespaced_lite = model_profile("vendor/gpt-5.6-sol-snapshot");
        assert!(namespaced_lite.responses_lite);
        assert_eq!(namespaced_lite.reasoning_effort, Some("low"));

        for model in [
            "vendor/group/gpt-5.6-sol-snapshot",
            "vendor!/gpt-5.4-mini-preview",
            "future-model",
        ] {
            let fallback = model_profile(model);
            assert!(!fallback.responses_lite, "model {model}");
            assert_eq!(fallback.reasoning_effort, None, "model {model}");
            assert_eq!(fallback.reasoning_summary, Some("auto"), "model {model}");
            assert_eq!(fallback.verbosity, None, "model {model}");
        }
    }

    #[test]
    fn effort_and_summary_defaults_are_independent_and_none_is_sticky() {
        let mut explicit_effort = serde_json::json!({
            "reasoning": {"effort": "high"}
        });
        merge_request_defaults(
            explicit_effort.as_object_mut().expect("object"),
            model_profile("gpt-5.2"),
        );
        assert_eq!(explicit_effort["reasoning"]["effort"], "high");
        assert_eq!(explicit_effort["reasoning"]["summary"], "auto");

        let mut disabled_summary = serde_json::json!({
            "reasoning": {"summary": "none"}
        });
        merge_request_defaults(
            disabled_summary.as_object_mut().expect("object"),
            model_profile("gpt-5.2"),
        );
        assert!(disabled_summary["reasoning"].get("summary").is_none());
        assert_eq!(disabled_summary["reasoning"]["effort"], "medium");
    }

    #[test]
    fn explicit_public_verbosity_remains_authoritative_for_unknown_models() {
        let mut request = serde_json::json!({"text": {"verbosity": "high"}});
        merge_request_defaults(
            request.as_object_mut().expect("object"),
            model_profile("future-model"),
        );
        assert_eq!(request["text"]["verbosity"], "high");
    }
}
