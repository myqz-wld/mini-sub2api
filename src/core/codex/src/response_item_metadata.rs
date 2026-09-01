use serde_json::Map;
use serde_json::Number;
use serde_json::Value;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub(crate) fn canonicalize_optionals(object: &mut Map<String, Value>) {
    if object.get("type").and_then(Value::as_str) == Some("compaction_summary") {
        object.insert("type".to_string(), Value::String("compaction".to_string()));
    }
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    match kind.as_deref() {
        Some("reasoning") => {
            object
                .entry("encrypted_content".to_string())
                .or_insert(Value::Null);
        }
        Some("local_shell_call") => {
            object.entry("call_id".to_string()).or_insert(Value::Null);
        }
        Some("tool_search_call") => {
            object.entry("call_id".to_string()).or_insert(Value::Null);
        }
        Some("tool_search_output") => {
            object.entry("call_id".to_string()).or_insert(Value::Null);
        }
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
        Some("search") => &["type", "queries", "query", "sources"],
        Some("open_page") => &["type", "url"],
        Some("find_in_page") => &["type", "url", "pattern"],
        Some("click") => &["type", "button", "x", "y", "keys"],
        Some("double_click" | "move") => &["type", "x", "y", "keys"],
        Some("drag") => &["type", "path", "keys"],
        Some("keypress") => &["type", "keys"],
        Some("scroll") => &["type", "scroll_x", "scroll_y", "x", "y", "keys"],
        Some("type") => &["type", "text"],
        Some("screenshot" | "wait") => &["type"],
        _ => &[
            "type",
            "command",
            "commands",
            "timeout_ms",
            "working_directory",
            "env",
            "user",
            "query",
            "queries",
            "sources",
            "url",
            "pattern",
            "text",
            "button",
            "x",
            "y",
            "keys",
            "path",
            "scroll_x",
            "scroll_y",
        ],
    };
    if let Some(sources) = action.get_mut("sources").and_then(Value::as_array_mut) {
        for source in sources {
            if let Some(source) = source.as_object_mut() {
                reorder(source, &["type", "url"]);
            }
        }
    }
    if let Some(path) = action.get_mut("path").and_then(Value::as_array_mut) {
        for point in path {
            if let Some(point) = point.as_object_mut() {
                reorder(point, &["x", "y"]);
            }
        }
    }
    reorder(action, order);
}

pub(crate) fn apply_missing_image_detail(value: &mut Value, detail: Option<&str>) {
    let Some(items) = value.as_array_mut() else {
        return;
    };
    for item in items {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        let content = match item.get("type").and_then(Value::as_str) {
            Some("message" | "agent_message") => item.get_mut("content"),
            Some("function_call_output" | "custom_tool_call_output") => item.get_mut("output"),
            _ => None,
        };
        let Some(content) = content.and_then(Value::as_array_mut) else {
            continue;
        };
        for block in content {
            let Some(block) = block.as_object_mut() else {
                continue;
            };
            if block.get("type").and_then(Value::as_str) == Some("input_image")
                && !block.contains_key("detail")
                && let Some(detail) = detail
            {
                block.insert("detail".to_string(), Value::String(detail.to_string()));
            }
        }
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

pub(crate) fn adds_create_time(object: &Map<String, Value>) -> bool {
    match object.get("type").and_then(Value::as_str) {
        Some("message") => matches!(
            object.get("role").and_then(Value::as_str),
            Some("user" | "system" | "developer")
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

fn reorder(object: &mut Map<String, Value>, order: &[&str]) {
    let mut existing = std::mem::take(object);
    for name in order {
        if let Some(value) = existing.remove(*name) {
            object.insert((*name).to_string(), value);
        }
    }
}
