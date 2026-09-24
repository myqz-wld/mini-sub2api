use serde_json::Map;
use serde_json::Value;

#[derive(Clone, Copy)]
pub(crate) struct ModelProfile {
    pub(crate) responses_lite: bool,
    pub(crate) node_repl_auto_review_required: bool,
    pub(crate) node_repl_disabled: bool,
    pub(crate) ultra_effort: &'static str,
    pub(crate) supports_verbosity: bool,
    pub(crate) original_images: bool,
    pub(crate) priority: bool,
    pub(crate) ultrafast: bool,
    reasoning_effort: Option<&'static str>,
    reasoning_summary: Option<&'static str>,
    verbosity: Option<&'static str>,
}

const MODEL_PROFILES: [(&str, ModelProfile); 9] = [
    (
        "gpt-6-astra",
        ModelProfile {
            node_repl_auto_review_required: true,
            ..profile(true, Some("low"), None, Some("low"))
        },
    ),
    (
        "gpt-daybreak-blue-latest",
        profile(true, Some("low"), None, Some("low")),
    ),
    (
        "gpt-daybreak-red-latest",
        profile(true, Some("medium"), None, Some("high")),
    ),
    ("gpt-5.6-sol", profile(true, Some("low"), None, Some("low"))),
    (
        "gpt-5.6-terra",
        profile(true, Some("medium"), None, Some("low")),
    ),
    (
        "gpt-5.6-luna",
        profile(true, Some("medium"), None, Some("low")),
    ),
    ("gpt-5.5", profile(false, Some("medium"), None, Some("low"))),
    ("gpt-5.4", profile(false, Some("medium"), None, Some("low"))),
    (
        "codex-auto-review",
        profile(true, Some("medium"), None, Some("low")),
    ),
];

const FALLBACK_PROFILE: ModelProfile = profile(false, None, Some("auto"), None);

pub(crate) fn model_profile(model: &str) -> ModelProfile {
    let mut profile = find_model_by_longest_prefix(model)
        .or_else(|| find_model_by_namespaced_suffix(model))
        .unwrap_or(FALLBACK_PROFILE);
    let label = diagnostic_model(model);
    profile.ultra_effort = match label {
        "gpt-6-astra" | "gpt-5.4" | "gpt-5.5" => "xhigh",
        "other" => "high",
        _ => "max",
    };
    profile.priority = !matches!(
        label,
        "other" | "gpt-daybreak-blue-latest" | "gpt-daybreak-red-latest"
    );
    profile.ultrafast = label == "gpt-5.6-sol";
    profile
}

pub(crate) fn diagnostic_model(model: &str) -> &'static str {
    let model = model.rsplit_once('/').map_or(model, |(_, suffix)| suffix);
    MODEL_PROFILES
        .iter()
        .filter(|(slug, _)| model.starts_with(slug))
        .max_by_key(|(slug, _)| slug.len())
        .map_or("other", |(slug, _)| *slug)
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
        ultra_effort: "high",
        supports_verbosity: verbosity.is_some(),
        original_images: verbosity.is_some(),
        priority: false,
        ultrafast: false,
        node_repl_auto_review_required: false,
        node_repl_disabled: false,
        reasoning_effort,
        reasoning_summary,
        verbosity,
    }
}

impl ModelProfile {
    pub(crate) fn supports_tier(self, tier: &str) -> bool {
        tier == "flex"
            || (tier == "priority" && self.priority)
            || (tier == "ultrafast" && self.ultrafast)
    }
}

pub(crate) fn merge_request_defaults(
    object: &mut Map<String, Value>,
    profile: ModelProfile,
    include_stream: bool,
) {
    merge_for_role(
        object,
        profile,
        include_stream,
        crate::native_request_policy::Role::Model,
    );
}

pub(crate) fn merge_for_role(
    object: &mut Map<String, Value>,
    profile: ModelProfile,
    include_stream: bool,
    role: crate::native_request_policy::Role,
) {
    crate::request_diagnostics::record_settings(object, false);
    crate::native_request_types::normalize(object);
    object
        .entry("store".to_string())
        .or_insert(Value::Bool(false));
    if include_stream {
        object
            .entry("stream".to_string())
            .or_insert(Value::Bool(true));
    }
    object
        .entry("tool_choice".to_string())
        .or_insert_with(|| Value::String("auto".to_string()));
    if profile.responses_lite {
        object.insert("parallel_tool_calls".to_string(), Value::Bool(false));
    } else {
        object
            .entry("parallel_tool_calls".to_string())
            .or_insert(Value::Bool(true));
    }
    merge_reasoning(object, profile);
    if role != crate::native_request_policy::Role::Classifier {
        merge_text(object, profile);
    }
    crate::native_request_policy::apply_controls(object, profile, role);
    crate::request_diagnostics::record_settings(object, true);
}

fn merge_reasoning(object: &mut Map<String, Value>, profile: ModelProfile) {
    if !object.contains_key("reasoning") {
        object.insert("reasoning".to_string(), Value::Object(Map::new()));
    }
    let Some(reasoning) = object.get_mut("reasoning").and_then(Value::as_object_mut) else {
        return;
    };
    if let Some(effort) = profile.reasoning_effort {
        reasoning
            .entry("effort".to_string())
            .or_insert_with(|| Value::String(effort.to_string()));
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
    if !object.contains_key("text") {
        object.insert("text".to_string(), Value::Object(Map::new()));
    }
    let Some(text) = object.get_mut("text").and_then(Value::as_object_mut) else {
        return;
    };
    if let Some(verbosity) = profile.verbosity {
        text.entry("verbosity".to_string())
            .or_insert_with(|| Value::String(verbosity.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_lookup_uses_longest_prefix_then_single_namespace_suffix() {
        let derived_mini = model_profile("gpt-5.4-mini-preview");
        assert_eq!(derived_mini.reasoning_effort, Some("medium"));
        assert_eq!(derived_mini.verbosity, Some("low"));

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
    fn effort_and_summary_defaults_are_independent_and_explicit_none_is_preserved() {
        let mut explicit_effort = serde_json::json!({
            "reasoning": {"effort": "high"}
        });
        merge_request_defaults(
            explicit_effort.as_object_mut().expect("object"),
            model_profile("gpt-5.2"),
            true,
        );
        assert_eq!(explicit_effort["reasoning"]["effort"], "high");
        assert_eq!(explicit_effort["reasoning"]["summary"], "auto");

        let mut disabled_summary = serde_json::json!({
            "reasoning": {"summary": "none"}
        });
        merge_request_defaults(
            disabled_summary.as_object_mut().expect("object"),
            model_profile("gpt-5.2"),
            true,
        );
        assert!(disabled_summary["reasoning"].get("summary").is_none());
        assert!(disabled_summary["reasoning"].get("effort").is_none());
    }

    #[test]
    fn explicit_null_controls_use_native_defaults() {
        let mut request = serde_json::json!({
            "store": true,
            "stream": false,
            "tool_choice": null,
            "parallel_tool_calls": true,
            "reasoning": null,
            "text": null,
            "include": null
        });
        let mut expected = request.clone();
        expected["tool_choice"] = "auto".into();
        expected["parallel_tool_calls"] = Value::Bool(false);
        expected["include"] = serde_json::json!(["reasoning.encrypted_content"]);
        expected["reasoning"] = serde_json::json!({"effort":"low", "context":"all_turns"});
        expected["text"] = serde_json::json!({"verbosity":"low"});
        merge_request_defaults(
            request.as_object_mut().expect("object"),
            model_profile("gpt-5.6-sol"),
            true,
        );
        assert_eq!(request, expected);
    }

    #[test]
    fn explicit_public_verbosity_remains_authoritative_for_unknown_models() {
        let mut request = serde_json::json!({"text": {"verbosity": "high"}});
        merge_request_defaults(
            request.as_object_mut().expect("object"),
            model_profile("future-model"),
            true,
        );
        assert!(request.get("text").is_none());
    }
}
