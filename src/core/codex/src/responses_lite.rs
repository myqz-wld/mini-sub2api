use serde_json::Map;
use serde_json::Value;
use uuid::Uuid;

#[path = "request_emulation_content.rs"]
mod content;
#[path = "request_emulation_items.rs"]
mod items;
#[path = "request_emulation_schema.rs"]
mod schema;
#[path = "request_emulation_tools.rs"]
mod tools;

use tools::canonical_tool;

const DEFAULT_NAMESPACE: &str = "functions";

pub(crate) fn group_tools(tools: Vec<Value>) -> Vec<Value> {
    let tools = tools.into_iter().map(canonical_tool).collect::<Vec<_>>();
    if let Some(namespace_index) = tools.iter().position(is_default_namespace) {
        let mut functions = Vec::new();
        let mut grouped = Vec::with_capacity(tools.len());
        for (index, tool) in tools.into_iter().enumerate() {
            if index != namespace_index && is_groupable_tool(&tool) {
                functions.push(tool);
            } else {
                grouped.push(tool);
            }
        }
        if !functions.is_empty()
            && let Some(namespace) = grouped.iter_mut().find(|tool| is_default_namespace(tool))
            && let Some(children) = namespace
                .as_object_mut()
                .and_then(|namespace| namespace.get_mut("tools"))
                .and_then(Value::as_array_mut)
        {
            children.extend(functions);
        }
        return grouped;
    }

    let mut functions = Vec::new();
    let mut functions_index = None;
    let mut grouped = Vec::new();
    for tool in tools {
        if is_groupable_tool(&tool) {
            functions_index.get_or_insert(grouped.len());
            functions.push(tool);
        } else {
            grouped.push(tool);
        }
    }
    if let Some(index) = functions_index.filter(|_| !functions.is_empty()) {
        grouped.insert(
            index,
            serde_json::json!({
                "type": "namespace",
                "name": DEFAULT_NAMESPACE,
                "description": "",
                "tools": functions,
            }),
        );
    }
    grouped
}

fn is_groupable_tool(tool: &Value) -> bool {
    matches!(
        tool.get("type").and_then(Value::as_str),
        Some("function" | "custom")
    )
}

fn is_default_namespace(tool: &Value) -> bool {
    tool.get("type").and_then(Value::as_str) == Some("namespace")
        && tool.get("name").and_then(Value::as_str) == Some(DEFAULT_NAMESPACE)
        && tool.get("tools").is_some_and(Value::is_array)
}

pub(crate) fn canonicalize_tools(tools: Vec<Value>) -> Vec<Value> {
    tools.into_iter().map(canonical_tool).collect()
}

pub(crate) fn assign_missing_item_ids(items: &mut [Value]) -> Vec<String> {
    let mut synthesized = Vec::new();
    for item in items {
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        items::canonicalize_item(object);
        if object.contains_key("id") {
            continue;
        }
        let Some(prefix) = object
            .get("type")
            .and_then(Value::as_str)
            .and_then(items::item_id_prefix)
        else {
            continue;
        };
        let id = Value::String(format!("{prefix}_{}", Uuid::now_v7()));
        if let Some(id) = id.as_str() {
            synthesized.push(id.to_string());
        }
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
    synthesized
}

pub(crate) fn canonicalize_request_items(
    request: &mut Map<String, Value>,
    default_image_detail: Option<&str>,
) {
    let turn_id = request
        .get("client_metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("turn_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(input) = request.get_mut("input") {
        canonicalize_input_items(input, default_image_detail, turn_id.as_deref());
    }
    if let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            *tool = canonical_tool(std::mem::take(tool));
        }
    }
    reorder_member(
        request,
        "reasoning",
        &["effort", "summary", "context", "generate_summary", "mode"],
    );
    reorder_member(
        request,
        "stream_options",
        &["include_obfuscation", "reasoning_summary_delivery"],
    );
    reorder_member(request, "text", &["verbosity", "format"]);
    if let Some(format) = request
        .get_mut("text")
        .and_then(Value::as_object_mut)
        .and_then(|text| text.get_mut("format"))
        .and_then(Value::as_object_mut)
    {
        if format.get("type").and_then(Value::as_str) == Some("json_schema") {
            format
                .entry("strict".to_string())
                .or_insert(Value::Bool(true));
            format
                .entry("name".to_string())
                .or_insert_with(|| Value::String("codex_output_schema".to_string()));
        }
        reorder(format, &["type", "strict", "schema", "name", "description"]);
        if let Some(schema) = format.get_mut("schema") {
            self::schema::canonicalize(schema);
        }
    }
}

pub(crate) fn canonicalize_injected_input(input: &mut Value) {
    canonicalize_input_items(input, None, None);
}

fn canonicalize_input_items(
    input: &mut Value,
    default_image_detail: Option<&str>,
    turn_id: Option<&str>,
) {
    if let Some(items) = input.as_array_mut() {
        for item in items {
            let Some(object) = item.as_object_mut() else {
                continue;
            };
            crate::response_item_metadata::stamp(object, turn_id);
            crate::response_item_metadata::strip_unprefixed_id(object);
            items::canonicalize_item(object);
        }
    }
    crate::response_item_metadata::apply_missing_image_detail(input, default_image_detail);
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

#[cfg(test)]
#[path = "responses_lite_tests.rs"]
mod tests;
