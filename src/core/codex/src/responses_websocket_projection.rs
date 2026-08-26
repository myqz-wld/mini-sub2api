use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use std::io;
use std::io::Write;

const VOLATILE_REQUEST_FIELDS: &[&str] = &[
    "client_metadata",
    "generate",
    "input",
    "previous_response_id",
    "stream_options",
];

const REUSABLE_ITEM_TYPES: &[&str] = &[
    "additional_tools",
    "agent_message",
    "apply_patch_call",
    "apply_patch_call_output",
    "code_interpreter_call",
    "compaction",
    "compaction_trigger",
    "computer_call",
    "computer_call_output",
    "context_compaction",
    "custom_tool_call",
    "custom_tool_call_output",
    "file_search_call",
    "function_call",
    "function_call_output",
    "image_generation_call",
    "local_shell_call",
    "local_shell_call_output",
    "mcp_approval_request",
    "mcp_approval_response",
    "mcp_call",
    "mcp_list_tools",
    "message",
    "other",
    "program",
    "program_output",
    "reasoning",
    "shell_call",
    "shell_call_output",
    "tool_search_call",
    "tool_search_output",
    "web_search_call",
];

pub(crate) fn project_properties(object: &Map<String, Value>) -> Map<String, Value> {
    object
        .iter()
        .filter(|(name, _)| !VOLATILE_REQUEST_FIELDS.contains(&name.as_str()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

pub(crate) fn equivalent_items(left: &[Value], right: &[Value]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| project_item(left) == project_item(right))
}

pub(crate) fn encoded_len_within(value: &Value, maximum: usize) -> Option<usize> {
    let mut counter = BoundedCounter {
        written: 0,
        maximum,
    };
    value
        .serialize(&mut serde_json::Serializer::new(&mut counter))
        .ok()?;
    Some(counter.written)
}

pub(crate) fn output_encoded_len(output: &[Value], maximum: usize) -> Option<usize> {
    let mut total = 0_usize;
    for item in output {
        total = total.checked_add(encoded_len_within(item, maximum.saturating_sub(total))?)?;
    }
    Some(total)
}

pub(crate) fn reusable_item(item: &Value) -> bool {
    item.as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|item_type| REUSABLE_ITEM_TYPES.contains(&item_type))
}

fn project_item(value: &Value) -> Value {
    let mut projected = value.clone();
    let Some(object) = projected.as_object_mut() else {
        return projected;
    };
    object.remove("id");
    object.remove("internal_chat_message_metadata_passthrough");
    projected
}

struct BoundedCounter {
    written: usize,
    maximum: usize,
}

impl Write for BoundedCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let next = self
            .written
            .checked_add(buffer.len())
            .filter(|next| *next <= self.maximum)
            .ok_or_else(|| io::Error::other("encoded value exceeds continuation budget"))?;
        self.written = next;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
