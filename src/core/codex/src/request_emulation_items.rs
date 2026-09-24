use super::content;
use super::tools;
use serde_json::Map;
use serde_json::Value;

pub(super) fn canonicalize_item(object: &mut Map<String, Value>) {
    crate::response_item_metadata::canonicalize_optionals(object);
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    canonicalize_nested(object, kind.as_deref());
    reorder(object, fields_for_kind(kind.as_deref()));
}

// Identity projection can add an ID after the semantic normalization pass. Reorder only
// the item envelope; never rebuild tools/schema bytes used by native deterministic IDs.
pub(super) fn order_projected_item(object: &mut Map<String, Value>) {
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    reorder_preserving(object, fields_for_kind(kind.as_deref()));
}

pub(super) fn item_id_prefix(kind: &str) -> Option<&'static str> {
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

fn canonicalize_nested(object: &mut Map<String, Value>, kind: Option<&str>) {
    if kind == Some("configuration_update")
        && let Some(reasoning) = object.get_mut("reasoning").and_then(Value::as_object_mut)
    {
        reorder(reasoning, &["effort"]);
    }
    canonicalize_contents(object, kind);
    if matches!(kind, Some("additional_tools" | "tool_search_output"))
        && let Some(entries) = object.get_mut("tools").and_then(Value::as_array_mut)
    {
        crate::native_request_policy::filter_tools(entries, false);
        for tool in entries {
            *tool = tools::canonical_tool(std::mem::take(tool));
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
        reorder_preserving(metadata, &["turn_id", "create_time", "executed_tool_calls"]);
    }
}

fn canonicalize_contents(object: &mut Map<String, Value>, kind: Option<&str>) {
    for name in ["content", "summary"] {
        if let Some(values) = object.get_mut(name).and_then(Value::as_array_mut) {
            content::filter(values, kind, name == "summary");
            for value in values {
                content::canonicalize(value);
            }
        }
    }
    if matches!(
        kind,
        Some("function_call_output" | "custom_tool_call_output")
    ) && let Some(values) = object.get_mut("output").and_then(Value::as_array_mut)
    {
        content::filter(values, kind, false);
        for value in values {
            content::canonicalize(value);
        }
    }
}

fn fields_for_kind(kind: Option<&str>) -> &'static [&'static str] {
    match kind {
        Some("configuration_update") => &["type", "reasoning"],
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
            "name",
            "namespace",
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
        Some("compaction" | "context_compaction") => &[
            "type",
            "id",
            "encrypted_content",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("compaction_trigger") => &["type"],
        Some("item_reference") => &["type", "id"],
        _ => &["type"],
    }
}

fn reorder(object: &mut Map<String, Value>, order: &[&str]) {
    crate::ignored_fields::retain(object, order, "input[].member");
    let mut existing = std::mem::take(object);
    for name in order {
        if let Some(value) = existing.remove(*name) {
            object.insert((*name).to_string(), value);
        }
    }
}

fn reorder_preserving(object: &mut Map<String, Value>, order: &[&str]) {
    let mut existing = std::mem::take(object);
    for name in order {
        if let Some(value) = existing.remove(*name) {
            object.insert((*name).to_string(), value);
        }
    }
    object.extend(existing);
}
