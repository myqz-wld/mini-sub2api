//! Caller history can omit output-only empty decoration and call-anchored item IDs.
//! This projection is only a lookup key: stored bodies, dependency checks and WS reuse stay exact.
use super::canonical;
use serde_json::Value;

pub(crate) fn candidate_key(item: &Value) -> Vec<u8> {
    let mut projected = item.clone();
    if let Some(object) = projected.as_object_mut() {
        if call_anchored(item) {
            object.remove("id");
            if object.get("status").and_then(Value::as_str) == Some("completed") {
                object.remove("status");
            }
        }
        if item.get("type").and_then(Value::as_str) == Some("message")
            && item.get("role").and_then(Value::as_str) == Some("assistant")
            && let Some(content) = object.get_mut("content").and_then(Value::as_array_mut)
        {
            for part in content {
                if part.get("type").and_then(Value::as_str) == Some("output_text")
                    && let Some(part) = part.as_object_mut()
                {
                    for field in ["annotations", "logprobs"] {
                        if part
                            .get(field)
                            .and_then(Value::as_array)
                            .is_some_and(Vec::is_empty)
                        {
                            part.remove(field);
                        }
                    }
                }
            }
        }
    }
    project_completed_metadata(&mut projected);
    canonical(&projected)
}

pub(crate) fn ids_compatible(request: &Value, saved: &Value) -> bool {
    let omitted_allowed = request.get("type").and_then(Value::as_str) == Some("message")
        || (call_anchored(request)
            && request.get("type") == saved.get("type")
            && request.get("call_id") == saved.get("call_id"));
    compare_ids(request, saved, omitted_allowed)
}

pub(crate) fn history_lookup_key(item: &Value) -> Vec<u8> {
    let mut item = item.clone();
    crate::reasoning_visibility::remove_ciphertext(&mut item);
    candidate_key(&item)
}

pub(crate) fn hidden_ciphertext_compatible(request: &Value, saved: &Value, hidden: bool) -> bool {
    if crate::reasoning_visibility::ciphertext(request)
        == crate::reasoning_visibility::ciphertext(saved)
    {
        return true;
    }
    if !hidden || crate::reasoning_visibility::ciphertext(request).is_some_and(|v| !v.is_null()) {
        return false;
    }
    // The index already compared every other semantic field. Missing ciphertext is compatible
    // only with a field actually suppressed by the gateway in this effective history.
    crate::reasoning_visibility::ciphertext(saved).is_some()
}

fn call_anchored(item: &Value) -> bool {
    item.get("type")
        .and_then(Value::as_str)
        .is_some_and(crate::subscription_request::uses_direct_call_reference)
        && item
            .get("call_id")
            .and_then(Value::as_str)
            .is_some_and(|id| {
                !id.trim().is_empty() && crate::request_state_types::validate_wire_id(id).is_ok()
            })
}

fn compare_ids(request: &Value, saved: &Value, omitted_allowed: bool) -> bool {
    if omitted_allowed && !request.as_object().is_some_and(|o| o.contains_key("id")) {
        return true;
    }
    request.get("id") == saved.get("id")
}

// A provider's item-done and terminal output are two reports of the same output, not a caller's
// reserialized history. Keep the original, stricter consistency contract for that consumer.
pub(crate) fn completion_items_compatible(observed: &Value, terminal: &Value) -> bool {
    completion_key(observed) == completion_key(terminal)
        && compare_ids(
            observed,
            terminal,
            observed.get("type").and_then(Value::as_str) == Some("message"),
        )
}

pub(crate) fn completion_key(item: &Value) -> Vec<u8> {
    let mut item = item.clone();
    project_completed_metadata(&mut item);
    canonical(&item)
}

fn project_completed_metadata(item: &mut Value) {
    if let Some(object) = item.as_object_mut() {
        object.remove("internal_chat_message_metadata_passthrough");
        if object.get("type").and_then(Value::as_str) == Some("message") {
            object.remove("id");
            if object.get("status").and_then(Value::as_str) == Some("completed") {
                object.remove("status");
            }
        }
    }
}
