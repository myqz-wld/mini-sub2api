use serde_json::Map;
use serde_json::Value;
use uuid::Uuid;
const DEFAULT_NAMESPACE: &str = "functions";

pub(crate) fn group_tools(tools: Vec<Value>) -> Vec<Value> {
    let mut functions = Vec::new();
    let mut functions_description = String::new();
    let mut functions_index = None;
    let mut grouped = Vec::new();
    for tool in tools.into_iter().map(canonical_tool) {
        let kind = tool.get("type").and_then(Value::as_str);
        match kind {
            Some("function" | "custom") => {
                functions_index.get_or_insert(grouped.len());
                functions.push(tool);
            }
            Some("namespace")
                if tool.get("name").and_then(Value::as_str) == Some(DEFAULT_NAMESPACE) =>
            {
                functions_index.get_or_insert(grouped.len());
                if let Some(description) = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .filter(|description| !description.trim().is_empty())
                {
                    functions_description = description.to_string();
                }
                if let Some(children) = tool.get("tools").and_then(Value::as_array) {
                    functions.extend(children.iter().cloned());
                }
            }
            _ => grouped.push(tool),
        }
    }
    if let Some(index) = functions_index.filter(|_| !functions.is_empty()) {
        grouped.insert(
            index,
            serde_json::json!({
                "type": "namespace",
                "name": DEFAULT_NAMESPACE,
                "description": functions_description,
                "tools": functions,
            }),
        );
    }
    grouped
}

pub(crate) fn canonicalize_tools(tools: Vec<Value>) -> Vec<Value> {
    tools.into_iter().map(canonical_tool).collect()
}

pub(crate) fn assign_missing_item_ids(items: &mut [Value]) {
    for item in items {
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        canonicalize_item(object);
        if let Some(id) = object.get("id").and_then(Value::as_str)
            && !id.is_empty()
        {
            continue;
        }
        let Some(prefix) = object
            .get("type")
            .and_then(Value::as_str)
            .and_then(item_id_prefix)
        else {
            continue;
        };
        let id = Value::String(format!("{prefix}_{}", Uuid::now_v7()));
        let existing = std::mem::take(object);
        let mut reordered = Map::new();
        for (name, value) in existing {
            let after_type = name == "type";
            reordered.insert(name, value);
            if after_type {
                reordered.insert("id".to_string(), id.clone());
            }
        }
        *object = reordered;
    }
}

pub(crate) fn canonicalize_request_items(request: &mut Map<String, Value>) {
    let turn_id = request
        .get("client_metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("turn_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(items) = request.get_mut("input").and_then(Value::as_array_mut) {
        for item in items {
            let Some(object) = item.as_object_mut() else {
                continue;
            };
            crate::response_item_metadata::stamp(object, turn_id.as_deref());
            crate::response_item_metadata::strip_unprefixed_id(object);
            canonicalize_item(object);
        }
    }
    if let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            *tool = canonical_tool(std::mem::take(tool));
        }
    }
    reorder_member(request, "reasoning", &["effort", "summary", "context"]);
    reorder_member(request, "stream_options", &["reasoning_summary_delivery"]);
    reorder_member(request, "text", &["verbosity", "format"]);
    if let Some(format) = request
        .get_mut("text")
        .and_then(Value::as_object_mut)
        .and_then(|text| text.get_mut("format"))
        .and_then(Value::as_object_mut)
    {
        if format.get("type").and_then(Value::as_str) == Some("json_schema") {
            if !format.get("strict").is_some_and(Value::is_boolean) {
                format.insert("strict".to_string(), Value::Bool(true));
            }
            format.insert(
                "name".to_string(),
                Value::String("codex_output_schema".to_string()),
            );
        }
        reorder(format, &["type", "strict", "schema", "name"]);
        if let Some(schema) = format.get_mut("schema") {
            canonicalize_schema(schema);
        }
    }
}

pub(crate) fn new_item_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::now_v7())
}

fn canonical_tool(mut tool: Value) -> Value {
    let Some(object) = tool.as_object_mut() else {
        return tool;
    };
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    for name in [
        "defer_loading",
        "external_web_access",
        "indexed_web_access",
        "filters",
        "user_location",
        "search_context_size",
        "search_content_types",
    ] {
        if object.get(name).is_some_and(Value::is_null) {
            object.remove(name);
        }
    }
    if kind.as_deref() == Some("function") {
        object
            .entry("description".to_string())
            .or_insert_with(|| Value::String(String::new()));
        object
            .entry("strict".to_string())
            .or_insert(Value::Bool(false));
        object
            .entry("parameters".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if kind.as_deref() == Some("namespace") {
        object
            .entry("description".to_string())
            .or_insert_with(|| Value::String(String::new()));
        if let Some(children) = object.get_mut("tools").and_then(Value::as_array_mut) {
            for child in children {
                *child = canonical_tool(std::mem::take(child));
            }
        }
    }
    if let Some(parameters) = object.get_mut("parameters") {
        canonicalize_schema(parameters);
    }
    if let Some(format) = object.get_mut("format").and_then(Value::as_object_mut) {
        reorder(format, &["type", "syntax", "definition"]);
    }
    if let Some(filters) = object.get_mut("filters").and_then(Value::as_object_mut) {
        if filters.get("allowed_domains").is_some_and(Value::is_null) {
            filters.remove("allowed_domains");
        }
        reorder(filters, &["allowed_domains"]);
    }
    if let Some(location) = object
        .get_mut("user_location")
        .and_then(Value::as_object_mut)
    {
        for name in ["country", "region", "city", "timezone"] {
            if location.get(name).is_some_and(Value::is_null) {
                location.remove(name);
            }
        }
        reorder(location, &["type", "country", "region", "city", "timezone"]);
    }
    let order: &[&str] = match kind.as_deref() {
        Some("function") => &[
            "type",
            "name",
            "description",
            "strict",
            "defer_loading",
            "parameters",
        ],
        Some("custom") => &["type", "name", "description", "defer_loading", "format"],
        Some("namespace") => &["type", "name", "description", "tools"],
        Some("tool_search") => &["type", "execution", "description", "parameters"],
        Some("web_search") => &[
            "type",
            "external_web_access",
            "indexed_web_access",
            "filters",
            "user_location",
            "search_context_size",
            "search_content_types",
        ],
        _ => return tool,
    };
    reorder(object, order);
    tool
}

fn canonicalize_item(object: &mut Map<String, Value>) {
    crate::response_item_metadata::canonicalize_optionals(object);
    let order: &[&str] = match object.get("type").and_then(Value::as_str) {
        Some("additional_tools") => &["type", "id", "role", "tools"],
        Some("message") => &[
            "type",
            "id",
            "role",
            "content",
            "phase",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("agent_message") => &[
            "type",
            "id",
            "author",
            "recipient",
            "content",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("function_call") => &[
            "type",
            "id",
            "name",
            "namespace",
            "arguments",
            "encrypted_function_args",
            "call_id",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("function_call_output") => &[
            "type",
            "id",
            "call_id",
            "output",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("custom_tool_call") => &[
            "type",
            "id",
            "status",
            "call_id",
            "name",
            "namespace",
            "input",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("custom_tool_call_output") => &[
            "type",
            "id",
            "call_id",
            "name",
            "output",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("reasoning") => &[
            "type",
            "id",
            "summary",
            "content",
            "encrypted_content",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("local_shell_call") => &[
            "type",
            "id",
            "call_id",
            "status",
            "action",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("tool_search_call") => &[
            "type",
            "id",
            "call_id",
            "status",
            "execution",
            "arguments",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("tool_search_output") => &[
            "type",
            "id",
            "call_id",
            "status",
            "execution",
            "tools",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("web_search_call") => &[
            "type",
            "id",
            "status",
            "action",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("image_generation_call") => &[
            "type",
            "id",
            "status",
            "revised_prompt",
            "result",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("compaction") => &[
            "type",
            "id",
            "encrypted_content",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("context_compaction") => &[
            "type",
            "id",
            "encrypted_content",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("compaction_trigger") => &["type"],
        _ => return,
    };
    if let Some(tools) = object.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            *tool = canonical_tool(std::mem::take(tool));
        }
    }
    for name in ["content", "summary", "output"] {
        if let Some(values) = object.get_mut(name).and_then(Value::as_array_mut) {
            for value in values {
                canonicalize_content(value);
            }
        }
    }
    if let Some(action) = object.get_mut("action").and_then(Value::as_object_mut) {
        crate::response_item_metadata::canonicalize_action(action);
    }
    if let Some(metadata) = object
        .get_mut("internal_chat_message_metadata_passthrough")
        .and_then(Value::as_object_mut)
    {
        crate::response_item_metadata::canonicalize_executed_tool_calls(metadata);
        reorder(metadata, &["turn_id", "create_time", "executed_tool_calls"]);
    }
    reorder(object, order);
}

fn canonicalize_content(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    crate::response_item_metadata::canonicalize_content_optionals(object);
    let order: &[&str] = match object.get("type").and_then(Value::as_str) {
        Some("input_text" | "output_text" | "summary_text" | "reasoning_text" | "text") => {
            &["type", "text"]
        }
        Some("input_image") => &["type", "image_url", "detail"],
        Some("input_audio") => &["type", "audio_url"],
        Some("encrypted_content") => &["type", "encrypted_content"],
        _ => return,
    };
    reorder(object, order);
}

fn canonicalize_schema(value: &mut Value) {
    let Some(schema) = value.as_object_mut() else {
        return;
    };
    for name in [
        "$ref",
        "type",
        "description",
        "encrypted",
        "enum",
        "items",
        "properties",
        "required",
        "additionalProperties",
        "anyOf",
        "oneOf",
        "allOf",
        "$defs",
        "definitions",
    ] {
        if schema.get(name).is_some_and(Value::is_null) {
            schema.remove(name);
        }
    }
    for name in ["items"] {
        if let Some(value) = schema.get_mut(name) {
            canonicalize_schema(value);
        }
    }
    if let Some(value) = schema
        .get_mut("additionalProperties")
        .filter(|value| value.is_object())
    {
        canonicalize_schema(value);
    }
    for name in ["anyOf", "oneOf", "allOf"] {
        if let Some(values) = schema.get_mut(name).and_then(Value::as_array_mut) {
            for value in values {
                canonicalize_schema(value);
            }
        }
    }
    for name in ["properties", "$defs", "definitions"] {
        if let Some(entries) = schema.get_mut(name).and_then(Value::as_object_mut) {
            for value in entries.values_mut() {
                canonicalize_schema(value);
            }
            let mut sorted = std::mem::take(entries).into_iter().collect::<Vec<_>>();
            sorted.sort_by(|left, right| left.0.cmp(&right.0));
            entries.extend(sorted);
        }
    }
    reorder(
        schema,
        &[
            "$ref",
            "type",
            "description",
            "encrypted",
            "enum",
            "items",
            "properties",
            "required",
            "additionalProperties",
            "anyOf",
            "oneOf",
            "allOf",
            "$defs",
            "definitions",
        ],
    );
}

fn reorder_member(object: &mut Map<String, Value>, name: &str, order: &[&str]) {
    if let Some(member) = object.get_mut(name).and_then(Value::as_object_mut) {
        reorder(member, order);
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

fn item_id_prefix(kind: &str) -> Option<&'static str> {
    match kind {
        "additional_tools" => Some("at"),
        "message" => Some("msg"),
        "agent_message" => Some("amsg"),
        "reasoning" => Some("rs"),
        "local_shell_call" => Some("lsh"),
        "function_call" => Some("fc"),
        "tool_search_call" => Some("tsc"),
        "function_call_output" => Some("fco"),
        "custom_tool_call" => Some("ctc"),
        "custom_tool_call_output" => Some("ctco"),
        "tool_search_output" => Some("tso"),
        "web_search_call" => Some("ws"),
        "image_generation_call" => Some("ig"),
        "compaction" | "context_compaction" => Some("cmp"),
        _ => None,
    }
}

#[cfg(test)]
#[path = "responses_lite_tests.rs"]
mod tests;
