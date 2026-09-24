//! Optional transport controls only. Required identities and opaque business values are untouched.
use crate::ignored_fields as log;
use serde_json::{Map, Value};

pub(crate) fn normalize(object: &mut Map<String, Value>) {
    for name in ["parallel_tool_calls", "store", "stream", "generate"] {
        keep(object, name, "request", Value::is_boolean);
    }
    keep(object, "instructions", "request", Value::is_string);
    for name in ["reasoning", "text", "stream_options", "access_programs"] {
        keep(object, name, "request", Value::is_object);
    }
    if let Some(reasoning) = object.get_mut("reasoning").and_then(Value::as_object_mut) {
        // Custom(String) effort values are part of the native protocol.
        keep(reasoning, "effort", "reasoning", |v| {
            v.as_str().is_some_and(|s| !s.is_empty())
        });
        keep(reasoning, "context", "reasoning", |v| {
            v.as_str() == Some("all_turns")
        });
    }
    if let Some(text) = object.get_mut("text").and_then(Value::as_object_mut) {
        keep(text, "verbosity", "text", |v| {
            matches!(v.as_str(), Some("low" | "medium" | "high"))
        });
        keep(text, "format", "text", Value::is_object);
    }
    if let Some(options) = object
        .get_mut("stream_options")
        .and_then(Value::as_object_mut)
    {
        keep(
            options,
            "reasoning_summary_delivery",
            "stream_options",
            |v| v.as_str() == Some("sequential_cutoff"),
        );
    }
    if let Some(programs) = object
        .get_mut("access_programs")
        .and_then(Value::as_object_mut)
    {
        keep(programs, "cyber", "access_programs", |v| {
            matches!(
                v.as_str(),
                Some("standard" | "daybreak_blue" | "daybreak_red")
            )
        });
        if !programs.contains_key("cyber") {
            log::remove(object, "access_programs", "request", "invalid_type");
        }
    }
}

fn keep(
    object: &mut Map<String, Value>,
    name: &'static str,
    path: &'static str,
    valid: impl Fn(&Value) -> bool,
) {
    if object.get(name).is_some_and(|value| !valid(value)) {
        log::remove(object, name, path, "invalid_type");
    }
}
