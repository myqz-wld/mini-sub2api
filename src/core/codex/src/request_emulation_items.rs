use super::content;
use super::tools;
use serde_json::Map;
use serde_json::Value;

const DOCUMENTED_ITEM_FIELDS: &[&str] = &[
    "type",
    "id",
    "status",
    "role",
    "phase",
    "agent",
    "content",
    "author",
    "recipient",
    "arguments",
    "call_id",
    "name",
    "namespace",
    "caller",
    "encrypted_function_args",
    "input",
    "output",
    "summary",
    "encrypted_content",
    "action",
    "actions",
    "execution",
    "environment",
    "fingerprint",
    "tools",
    "revised_prompt",
    "result",
    "results",
    "queries",
    "pending_safety_checks",
    "acknowledged_safety_checks",
    "approval_request_id",
    "approve",
    "reason",
    "server_label",
    "error",
    "code",
    "container_id",
    "outputs",
    "operation",
    "program",
    "max_output_length",
    "created_by",
    "internal_chat_message_metadata_passthrough",
];

pub(super) fn canonicalize_item(object: &mut Map<String, Value>) {
    crate::response_item_metadata::canonicalize_optionals(object);
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    canonicalize_nested(object, kind.as_deref());
    reorder(object, fields_for_kind(kind.as_deref()));
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
    canonicalize_contents(object, kind);
    canonicalize_tools(object, kind);
    canonicalize_caller(object);
    canonicalize_agent(object);
    canonicalize_actions(object, kind);
    canonicalize_safety_checks(object);
    canonicalize_file_results(object);
    canonicalize_operation(object);
    canonicalize_structured_outputs(object, kind);
    if let Some(environment) = object.get_mut("environment").and_then(Value::as_object_mut) {
        tools::canonicalize_environment(environment);
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
        for value in values {
            content::canonicalize(value);
        }
    }
}

fn canonicalize_tools(object: &mut Map<String, Value>, kind: Option<&str>) {
    let Some(entries) = object.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };
    if kind == Some("mcp_list_tools") {
        for entry in entries {
            if let Some(entry) = entry.as_object_mut() {
                reorder(
                    entry,
                    &["name", "input_schema", "annotations", "description"],
                );
            }
        }
        return;
    }
    for tool in entries {
        *tool = tools::canonical_tool(std::mem::take(tool));
    }
}

fn canonicalize_caller(object: &mut Map<String, Value>) {
    if let Some(caller) = object.get_mut("caller").and_then(Value::as_object_mut) {
        reorder(caller, &["type", "caller_id"]);
    }
}

fn canonicalize_agent(object: &mut Map<String, Value>) {
    if let Some(agent) = object.get_mut("agent").and_then(Value::as_object_mut) {
        reorder(agent, &["agent_name"]);
    }
}

fn canonicalize_actions(object: &mut Map<String, Value>, kind: Option<&str>) {
    if let Some(action) = object.get_mut("action").and_then(Value::as_object_mut) {
        if kind == Some("shell_call") {
            reorder(action, &["commands", "max_output_length", "timeout_ms"]);
        } else {
            crate::response_item_metadata::canonicalize_action(action);
        }
    }
    if let Some(actions) = object.get_mut("actions").and_then(Value::as_array_mut) {
        for action in actions {
            if let Some(action) = action.as_object_mut() {
                crate::response_item_metadata::canonicalize_action(action);
            }
        }
    }
}

fn canonicalize_safety_checks(object: &mut Map<String, Value>) {
    for name in ["pending_safety_checks", "acknowledged_safety_checks"] {
        if let Some(checks) = object.get_mut(name).and_then(Value::as_array_mut) {
            for check in checks {
                if let Some(check) = check.as_object_mut() {
                    reorder(check, &["id", "code", "message"]);
                }
            }
        }
    }
}

fn canonicalize_file_results(object: &mut Map<String, Value>) {
    if let Some(results) = object.get_mut("results").and_then(Value::as_array_mut) {
        for result in results {
            if let Some(result) = result.as_object_mut() {
                reorder(
                    result,
                    &["attributes", "file_id", "filename", "score", "text"],
                );
            }
        }
    }
}

fn canonicalize_operation(object: &mut Map<String, Value>) {
    let Some(operation) = object.get_mut("operation").and_then(Value::as_object_mut) else {
        return;
    };
    let fields: &[&str] = match operation.get("type").and_then(Value::as_str) {
        Some("create_file" | "update_file") => &["type", "path", "diff"],
        Some("delete_file") => &["type", "path"],
        _ => &["type", "path", "diff"],
    };
    reorder(operation, fields);
}

fn canonicalize_structured_outputs(object: &mut Map<String, Value>, kind: Option<&str>) {
    if kind == Some("shell_call_output")
        && let Some(outputs) = object.get_mut("output").and_then(Value::as_array_mut)
    {
        for output in outputs {
            let Some(output) = output.as_object_mut() else {
                continue;
            };
            if let Some(outcome) = output.get_mut("outcome").and_then(Value::as_object_mut) {
                reorder(outcome, &["type", "exit_code"]);
            }
            reorder(output, &["outcome", "stderr", "stdout", "created_by"]);
        }
    }
    if kind == Some("computer_call_output")
        && let Some(output) = object.get_mut("output").and_then(Value::as_object_mut)
    {
        reorder(output, &["type", "file_id", "image_url"]);
    }
    if kind == Some("code_interpreter_call")
        && let Some(outputs) = object.get_mut("outputs").and_then(Value::as_array_mut)
    {
        for output in outputs {
            if let Some(output) = output.as_object_mut() {
                reorder(output, &["type", "logs", "url"]);
            }
        }
    }
    if kind == Some("mcp_call")
        && let Some(error) = object.get_mut("error").and_then(Value::as_object_mut)
    {
        reorder(error, &["type", "code", "message", "content"]);
    }
}

fn fields_for_kind(kind: Option<&str>) -> &'static [&'static str] {
    match kind {
        Some("additional_tools") => &["type", "id", "role", "tools"],
        Some("message") => &[
            "type",
            "id",
            "role",
            "content",
            "status",
            "phase",
            "agent",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("agent_message") => &[
            "type",
            "id",
            "author",
            "recipient",
            "content",
            "agent",
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
            "caller",
            "status",
            "agent",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("function_call_output") => &[
            "type",
            "id",
            "call_id",
            "name",
            "namespace",
            "output",
            "caller",
            "status",
            "agent",
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
            "caller",
            "agent",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("custom_tool_call_output") => &[
            "type",
            "id",
            "call_id",
            "name",
            "namespace",
            "output",
            "caller",
            "status",
            "agent",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("reasoning") => &[
            "type",
            "id",
            "summary",
            "content",
            "encrypted_content",
            "status",
            "agent",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("file_search_call") => &["type", "id", "queries", "status", "results", "agent"],
        Some("computer_call") => &[
            "type",
            "id",
            "call_id",
            "action",
            "actions",
            "pending_safety_checks",
            "status",
            "agent",
        ],
        Some("computer_call_output") => &[
            "type",
            "id",
            "call_id",
            "output",
            "acknowledged_safety_checks",
            "status",
            "agent",
            "created_by",
        ],
        Some("local_shell_call") => &["type", "id", "call_id", "status", "action"],
        Some("local_shell_call_output") => &["type", "id", "output", "status"],
        Some("shell_call") => &[
            "type",
            "id",
            "call_id",
            "action",
            "caller",
            "environment",
            "status",
            "created_by",
            "agent",
        ],
        Some("shell_call_output") => &[
            "type",
            "id",
            "call_id",
            "output",
            "caller",
            "status",
            "max_output_length",
            "created_by",
            "agent",
        ],
        Some("apply_patch_call") => &[
            "type",
            "id",
            "call_id",
            "operation",
            "caller",
            "status",
            "created_by",
            "agent",
        ],
        Some("apply_patch_call_output") => &[
            "type",
            "id",
            "call_id",
            "output",
            "caller",
            "status",
            "created_by",
            "agent",
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
            "agent",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("code_interpreter_call") => {
            &["type", "id", "code", "container_id", "outputs", "status"]
        }
        Some("image_generation_call") => &[
            "type",
            "id",
            "status",
            "revised_prompt",
            "result",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("mcp_list_tools") => &["type", "id", "server_label", "tools", "error"],
        Some("mcp_approval_request") => &["type", "id", "arguments", "name", "server_label"],
        Some("mcp_approval_response") => {
            &["type", "id", "approval_request_id", "approve", "reason"]
        }
        Some("mcp_call") => &[
            "type",
            "id",
            "arguments",
            "name",
            "server_label",
            "error",
            "output",
            "status",
            "approval_request_id",
        ],
        Some("program") => &[
            "type",
            "id",
            "call_id",
            "code",
            "status",
            "environment",
            "fingerprint",
        ],
        Some("program_output") => &["type", "id", "call_id", "result", "status"],
        Some("compaction" | "context_compaction") => &[
            "type",
            "id",
            "encrypted_content",
            "created_by",
            "agent",
            "internal_chat_message_metadata_passthrough",
        ],
        Some("compaction_trigger" | "item_reference") => &["type", "id", "agent"],
        _ => DOCUMENTED_ITEM_FIELDS,
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

fn reorder_preserving(object: &mut Map<String, Value>, order: &[&str]) {
    let mut existing = std::mem::take(object);
    for name in order {
        if let Some(value) = existing.remove(*name) {
            object.insert((*name).to_string(), value);
        }
    }
    object.extend(existing);
}
