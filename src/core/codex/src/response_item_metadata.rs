use serde_json::Map;
use serde_json::Number;
use serde_json::Value;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub(crate) fn canonicalize_optionals(object: &mut Map<String, Value>) {
    remove_null(object, "id");
    remove_null(object, "internal_chat_message_metadata_passthrough");
    if object.get("type").and_then(Value::as_str) == Some("compaction_summary") {
        object.insert("type".to_string(), Value::String("compaction".to_string()));
    }
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    match kind.as_deref() {
        Some("message") => remove_null(object, "phase"),
        Some("reasoning") => {
            if object.get("content").is_some_and(|content| {
                content.is_null()
                    || content.as_array().is_some_and(|content| {
                        content.iter().any(|item| {
                            item.get("type").and_then(Value::as_str) == Some("reasoning_text")
                        })
                    })
            }) {
                object.remove("content");
            }
            object
                .entry("encrypted_content".to_string())
                .or_insert(Value::Null);
        }
        Some("local_shell_call") => {
            object.entry("call_id".to_string()).or_insert(Value::Null);
        }
        Some("function_call") => {
            remove_null(object, "namespace");
            remove_null(object, "encrypted_function_args");
        }
        Some("tool_search_call") => {
            object.entry("call_id".to_string()).or_insert(Value::Null);
            remove_null(object, "status");
        }
        Some("custom_tool_call") => {
            remove_null(object, "status");
            remove_null(object, "namespace");
        }
        Some("custom_tool_call_output") => remove_null(object, "name"),
        Some("tool_search_output") => {
            object.entry("call_id".to_string()).or_insert(Value::Null);
        }
        Some("web_search_call") => {
            remove_null(object, "status");
            remove_null(object, "action");
        }
        Some("image_generation_call") => remove_null(object, "revised_prompt"),
        Some("context_compaction") => remove_null(object, "encrypted_content"),
        _ => {}
    }
}

pub(crate) fn canonicalize_action(action: &mut Map<String, Value>) {
    let order: &[&str] = match action.get("type").and_then(Value::as_str) {
        Some("exec") => {
            for name in ["timeout_ms", "working_directory", "env", "user"] {
                action.entry(name.to_string()).or_insert(Value::Null);
            }
            &[
                "type",
                "command",
                "timeout_ms",
                "working_directory",
                "env",
                "user",
            ]
        }
        Some("search") => {
            remove_null(action, "query");
            remove_null(action, "queries");
            &["type", "query", "queries"]
        }
        Some("open_page") => {
            remove_null(action, "url");
            &["type", "url"]
        }
        Some("find_in_page") => {
            remove_null(action, "url");
            remove_null(action, "pattern");
            &["type", "url", "pattern"]
        }
        _ => return,
    };
    reorder(action, order);
}

pub(crate) fn canonicalize_content_optionals(object: &mut Map<String, Value>) {
    if object.get("type").and_then(Value::as_str) == Some("input_image") {
        remove_null(object, "detail");
    }
}

pub(crate) fn canonicalize_executed_tool_calls(metadata: &mut Map<String, Value>) {
    let Some(calls) = metadata
        .get_mut("executed_tool_calls")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for call in calls {
        let Some(call) = call.as_object_mut() else {
            continue;
        };
        reorder(call, &["name", "arguments"]);
        let Some(truncation) = call
            .get_mut("arguments")
            .and_then(Value::as_object_mut)
            .and_then(|arguments| {
                arguments
                    .get_mut("_codex_executed_tool_call_truncated")
                    .and_then(Value::as_object_mut)
            })
        else {
            continue;
        };
        remove_null(truncation, "omitted_calls");
        remove_null(truncation, "original_name_bytes");
        reorder(
            truncation,
            &[
                "original_bytes",
                "max_bytes",
                "omitted_calls",
                "original_name_bytes",
            ],
        );
    }
}

pub(crate) fn stamp(object: &mut Map<String, Value>, turn_id: Option<&str>) {
    let Some(turn_id) = turn_id else {
        return;
    };
    let kind = object.get("type").and_then(Value::as_str);
    if kind == Some("additional_tools")
        || kind == Some("compaction_trigger")
        || object
            .get("id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return;
    }
    let add_create_time = adds_create_time(object);
    let metadata = object
        .entry("internal_chat_message_metadata_passthrough".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !metadata.is_object() {
        *metadata = Value::Object(Map::new());
    }
    let metadata = metadata.as_object_mut().expect("item metadata object");
    if metadata
        .get("turn_id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        metadata.insert("turn_id".to_string(), Value::String(turn_id.to_string()));
    }
    if add_create_time && !metadata.get("create_time").is_some_and(Value::is_number) {
        metadata.insert("create_time".to_string(), current_create_time());
    }
}

pub(crate) fn strip_unprefixed_id(object: &mut Map<String, Value>) {
    if object.get("id").and_then(Value::as_str).is_some_and(|id| {
        !id.is_empty()
            && !id
                .split_once('_')
                .is_some_and(|(prefix, suffix)| !prefix.is_empty() && !suffix.is_empty())
    }) {
        object.remove("id");
    }
}

fn adds_create_time(object: &Map<String, Value>) -> bool {
    match object.get("type").and_then(Value::as_str) {
        Some("message") => matches!(
            object.get("role").and_then(Value::as_str),
            Some("user" | "developer")
        ),
        Some(
            "agent_message"
            | "function_call_output"
            | "custom_tool_call_output"
            | "tool_search_output",
        ) => true,
        _ => false,
    }
}

fn current_create_time() -> Value {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Number::from_f64(duration.as_secs_f64())
        .map(Value::Number)
        .unwrap_or_else(|| Value::Number(duration.as_secs().into()))
}

fn remove_null(object: &mut Map<String, Value>, name: &str) {
    if object.get(name).is_some_and(Value::is_null) {
        object.remove(name);
    }
}

fn reorder(object: &mut Map<String, Value>, order: &[&str]) {
    let mut existing = std::mem::take(object);
    for name in order {
        if let Some(value) = existing.remove(*name) {
            object.insert((*name).to_string(), value);
        }
    }
}
