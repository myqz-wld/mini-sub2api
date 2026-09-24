//! Codex 0.156.0 ModelClient and Guardian policies, separate from public API passthrough.
use crate::ignored_fields as log;
use http::HeaderMap;
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Model,
    Reviewer,
    Classifier,
    Memory,
}
impl Role {
    pub(crate) fn read(object: &Map<String, Value>, headers: &HeaderMap) -> Self {
        match headers
            .get("x-codex-guardian")
            .and_then(|v| v.to_str().ok())
        {
            Some("classifier") => return Self::Classifier,
            Some("reviewer") => return Self::Reviewer,
            _ => {}
        }
        let raw = object
            .get("client_metadata")
            .and_then(|m| m.get("x-codex-turn-metadata"))
            .and_then(Value::as_str)
            .or_else(|| {
                headers
                    .get("x-codex-turn-metadata")
                    .and_then(|v| v.to_str().ok())
            });
        if raw
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .is_some_and(|v| v["request_kind"] == "memory")
        {
            Self::Memory
        } else {
            Self::Model
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Reviewer => "reviewer",
            Self::Classifier => "classifier",
            Self::Memory => "memory",
        }
    }
}

// These types are distinct from ToolSpec. item_reference is a local, ownership-checked control.
pub(crate) fn supported_item(kind: &str) -> bool {
    matches!(
        kind,
        "additional_tools"
            | "message"
            | "agent_message"
            | "reasoning"
            | "local_shell_call"
            | "function_call"
            | "function_call_output"
            | "custom_tool_call"
            | "custom_tool_call_output"
            | "tool_search_call"
            | "tool_search_output"
            | "web_search_call"
            | "image_generation_call"
            | "compaction"
            | "compaction_summary"
            | "context_compaction"
            | "configuration_update"
            | "compaction_trigger"
            | "item_reference"
    )
}

pub(crate) fn filter_admission(object: &mut Map<String, Value>) {
    if let Some(programs) = object
        .get_mut("access_programs")
        .and_then(Value::as_object_mut)
    {
        log::retain(programs, &["cyber"], "access_programs");
    }
    if let Some(tools) = object.get_mut("tools").and_then(Value::as_array_mut) {
        filter_tools(tools, false);
    }
    if let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) {
        items.retain_mut(|item| {
            // Leave malformed required structures for the existing fail-closed validation.
            let Some(kind) = item.get("type").and_then(Value::as_str) else {
                return true;
            };
            if !supported_item(kind) {
                log::record("input[]", "type", "unsupported_variant");
                return false;
            }
            if matches!(kind, "additional_tools" | "tool_search_output")
                && let Some(tools) = item.get_mut("tools").and_then(Value::as_array_mut)
            {
                filter_tools(tools, false);
            }
            true
        });
    }
}

pub(crate) fn filter_tools(tools: &mut Vec<Value>, namespace: bool) {
    tools.retain_mut(|tool| {
        let Some(kind) = tool.get("type").and_then(Value::as_str) else {
            return true;
        };
        let supported = matches!(kind, "function" | "custom")
            || (!namespace && matches!(kind, "namespace" | "web_search" | "tool_search"));
        if !supported {
            log::record("tools[]", "type", "unsupported_variant");
            return false;
        }
        if kind == "namespace"
            && let Some(children) = tool.get_mut("tools").and_then(Value::as_array_mut)
        {
            filter_tools(children, true);
        }
        true
    });
}

pub(crate) fn apply_controls(
    object: &mut Map<String, Value>,
    profile: crate::request_defaults::ModelProfile,
    role: Role,
) {
    let choice = if role == Role::Classifier {
        "none"
    } else {
        "auto"
    };
    if object
        .get("tool_choice")
        .is_some_and(|v| v.as_str() != Some(choice))
    {
        log::record("request", "tool_choice", "role_policy");
    }
    object.insert("tool_choice".into(), choice.into());
    let include = if role == Role::Classifier {
        json!([])
    } else {
        json!(["reasoning.encrypted_content"])
    };
    if object.get("include").is_some_and(|v| v != &include) {
        log::record("request", "include", "role_policy");
    }
    object.insert("include".into(), include);
    if role == Role::Classifier {
        log::remove(object, "text", "request", "role_policy");
        object.insert("parallel_tool_calls".into(), false.into());
    }
    if let Some(tier) = object.get("service_tier")
        && (role == Role::Reviewer
            || tier
                .as_str()
                .is_none_or(|tier| !profile.supports_tier(tier)))
    {
        log::remove(object, "service_tier", "request", "unsupported_tier");
    }
    if let Some(reasoning) = object.get_mut("reasoning").and_then(Value::as_object_mut) {
        log::retain(reasoning, &["effort", "summary", "context"], "reasoning");
        if let Some(effort) = reasoning.get("effort").and_then(Value::as_str) {
            let resolved = match effort {
                "persistent" => Some("disabled"),
                "ultra" => Some(profile.ultra_effort),
                _ => None,
            };
            if let Some(resolved) = resolved {
                reasoning.insert("effort".into(), resolved.into());
            }
        }
        if reasoning
            .get("summary")
            .is_some_and(|v| !matches!(v.as_str(), Some("auto" | "concise" | "detailed")))
        {
            log::remove(reasoning, "summary", "reasoning", "disabled_summary");
        }
        if profile.responses_lite {
            reasoning.insert("context".into(), "all_turns".into());
        } else {
            log::remove(reasoning, "context", "reasoning", "model_policy");
        }
    }
    if object
        .get("reasoning")
        .and_then(|r| r.get("summary"))
        .is_none()
    {
        log::remove(object, "stream_options", "request", "summary_disabled");
    }
    if let Some(text) = object.get_mut("text").and_then(Value::as_object_mut) {
        log::retain(text, &["verbosity", "format"], "text");
        if !profile.supports_verbosity {
            log::remove(text, "verbosity", "text", "model_policy");
        }
        if let Some(format) = text.get_mut("format").and_then(Value::as_object_mut) {
            if format.get("type").and_then(Value::as_str) == Some("json_schema")
                && format.contains_key("schema")
            {
                log::retain(format, &["type", "strict", "schema", "name"], "text.format");
                if format
                    .get("name")
                    .is_some_and(|v| v != "codex_output_schema")
                {
                    log::record("text.format", "name", "role_policy");
                }
                format.insert("name".into(), "codex_output_schema".into());
                if role != Role::Reviewer || !format.get("strict").is_some_and(Value::is_boolean) {
                    if format.get("strict") == Some(&Value::Bool(false)) {
                        log::record("text.format", "strict", "role_policy");
                    }
                    format.insert("strict".into(), true.into());
                }
            } else {
                log::remove(text, "format", "text", "unsupported_variant");
            }
        }
        if text.is_empty() {
            object.shift_remove("text");
        }
    }
}

// Fixed native default: reasoning_effort_override is off; the bundled catalog also does not
// advertise updates. Local item references also have no native ResponseItem representation.
// Filter only AFTER admission, metadata capture and required-reference identity validation.
pub(crate) fn filter_send_only(
    object: &mut Map<String, Value>,
    transport: crate::request_normalizer::EmulationTransport,
) {
    object.shift_remove("conversation");
    if transport == crate::request_normalizer::EmulationTransport::Http {
        object.shift_remove("previous_response_id");
    }
    if let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) {
        items.retain(|item| {
            !matches!(
                item.get("type").and_then(Value::as_str),
                Some("configuration_update" | "item_reference")
            )
        });
    }
}
