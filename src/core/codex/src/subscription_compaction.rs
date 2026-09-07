//! Memory-only replacement windows; opaque compaction content is never interpreted.
use crate::request_compaction::CompactionOutput;
use crate::subscription_context::Record;
use crate::subscription_index::Item;
use crate::subscription_request::{Dependencies, Format};
use serde_json::Value;
use std::ops::Range;
use std::sync::Arc;

pub(super) enum Window {
    Append,
    Replace {
        input: Vec<Arc<Item>>,
        output: Range<usize>,
    },
    Unavailable,
}

pub(super) fn select(record: &Record, observed: &CompactionOutput, output: &[Value]) -> Window {
    let mut compacted = output
        .iter()
        .enumerate()
        .filter(|(_, item)| item.get("type").and_then(Value::as_str) == Some("compaction"));
    let first = compacted.next();
    if first.is_none() && record.identity.request_kind != "compaction" {
        return Window::Append;
    }
    let Some(history) = &record.history else {
        // Receiving a suffix/checkpoint does not retroactively prove missing source context.
        return Window::Unavailable;
    };
    let Some((index, checkpoint)) = first else {
        // Local text summaries require the client's summary wrapper and replacement decisions.
        return Window::Unavailable;
    };
    if compacted.next().is_some() || !observed.matches_single(checkpoint) {
        return Window::Unavailable;
    }
    let source = history.items();
    let explicit = record.identity.request_kind == "compaction"
        || source
            .iter()
            .any(|item| kind(&item.value) == "compaction_trigger");
    let retained = if explicit {
        let Some(retained) = retain_explicit(&source, record.caller_format) else {
            return Window::Unavailable;
        };
        retained
    } else if record.caller_format == Format::Lite {
        let Some(prefix) = retain_lite_setup(&source) else {
            return Window::Unavailable;
        };
        prefix
    } else {
        Vec::new()
    };
    // V2 compaction consumes only the checkpoint. In-band generation may also emit later
    // assistant/tool items, which belong to the new window and must remain in their exact order.
    let range = index..if explicit { index + 1 } else { output.len() };
    let mut dependencies = Dependencies::default();
    for item in retained
        .iter()
        .map(|item| &item.value)
        .chain(&output[range.clone()])
    {
        if dependencies.append(std::slice::from_ref(item)).is_err() {
            return Window::Unavailable;
        }
    }
    if record
        .dependencies
        .calls
        .iter()
        .any(|(call, consumed)| !consumed && !dependencies.calls.contains_key(call))
    {
        // An unresolved call cannot disappear into an opaque local reconstruction.
        return Window::Unavailable;
    }
    Window::Replace {
        input: retained,
        output: range,
    }
}

fn kind(item: &Value) -> &str {
    item.get("type").and_then(Value::as_str).unwrap_or("")
}

fn retain_lite_setup(source: &[&Arc<Item>]) -> Option<Vec<Arc<Item>>> {
    if source
        .first()
        .is_none_or(|item| kind(&item.value) != "additional_tools")
    {
        return None;
    }
    // Lite configuration lives in input. Preserve the caller's leading tools/developer block;
    // the first developer item need not be a base instruction, so do not infer a fixed count.
    Some(
        source
            .iter()
            .take_while(|item| {
                kind(&item.value) == "additional_tools"
                    || (kind(&item.value) == "message"
                        && matches!(
                            item.value.get("role").and_then(Value::as_str),
                            Some("developer" | "system")
                        ))
            })
            .map(|item| Arc::clone(item))
            .collect(),
    )
}

fn retain_explicit(source: &[&Arc<Item>], format: Format) -> Option<Vec<Arc<Item>>> {
    if source
        .last()
        .is_none_or(|item| kind(&item.value) != "compaction_trigger")
    {
        return None;
    }
    let mut retained = Vec::with_capacity(source.len());
    for (index, item) in source[..source.len() - 1].iter().enumerate() {
        match kind(&item.value) {
            "additional_tools" if index == 0 && format == Format::Lite => {
                retained.push(Arc::clone(item));
            }
            "message" => match item.value.get("role").and_then(Value::as_str) {
                // Preserve the caller's visible instructions/environment without inventing the
                // native client's untransmitted hooks, fresh environment or truncation settings.
                Some("user" | "system" | "developer") => retained.push(Arc::clone(item)),
                Some("assistant") => {}
                _ => return None,
            },
            "reasoning"
            | "compaction"
            | "context_compaction"
            | "function_call"
            | "function_call_output"
            | "custom_tool_call"
            | "custom_tool_call_output"
            | "local_shell_call"
            | "local_shell_call_output"
            | "shell_call"
            | "shell_call_output"
            | "computer_call"
            | "computer_call_output"
            | "web_search_call"
            | "file_search_call"
            | "code_interpreter_call"
            | "image_generation_call"
            | "tool_search_call"
            | "tool_search_output"
            | "apply_patch_call"
            | "apply_patch_call_output" => {}
            // Unknown context, external item references, repeated triggers and unsupported tool
            // relationships need an explicit replacement rather than silent content loss.
            _ => return None,
        }
    }
    Some(retained)
}
