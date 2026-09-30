//! Codex 0.159.2 observation budgets; model-visible content is never trimmed.
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

#[path = "tool_observation_shedding.rs"]
mod shedding;

pub(crate) const META: &str = "internal_chat_message_metadata_passthrough";
const CALLS: &str = "executed_tool_calls";
const COMPLETE: &str = "tool_calls_complete";
const TRUNCATED: &str = "_codex_executed_tool_call_truncated";
const ARGUMENT_BYTES: usize = 8 * 1024;
pub(crate) const PROMPT_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MESSAGE_BYTES: usize = 15 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Origin {
    pub(crate) cell: Option<String>,
    pub(crate) item: Option<String>,
    pub(crate) call: Option<String>,
    pub(crate) turn: Option<String>,
}

pub(crate) fn origin(item: &Value) -> Origin {
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    Origin {
        cell: text(item.get(META).and_then(|m| m.get("cell_id"))),
        item: text(item.get("id")),
        call: text(item.get("call_id")),
        turn: text(item.get(META).and_then(|m| m.get("turn_id"))),
    }
}

fn size(value: &impl serde::Serialize) -> usize {
    crate::json_size::encoded_len(value).unwrap_or(usize::MAX)
}

fn metadata(item: &Value) -> Option<&Map<String, Value>> {
    item.get(META)?.as_object()
}
fn metadata_mut(item: &mut Value) -> Option<&mut Map<String, Value>> {
    item.get_mut(META)?.as_object_mut()
}
fn calls(item: &Value) -> Option<&Vec<Value>> {
    metadata(item)?.get(CALLS)?.as_array()
}
fn calls_mut(item: &mut Value) -> Option<&mut Vec<Value>> {
    metadata_mut(item)?.get_mut(CALLS)?.as_array_mut()
}

pub(crate) fn observation_bytes(item: &Value) -> usize {
    let Some(metadata) = metadata(item) else {
        return 0;
    };
    let mut bytes: usize = 0;
    let mut fields: usize = 0;
    for key in ["cell_id", CALLS, COMPLETE] {
        if let Some(value) = metadata.get(key) {
            bytes = bytes
                .saturating_add(size(&key) + 1)
                .saturating_add(size(value));
            fields += 1;
        }
    }
    if fields == 0 {
        return 0;
    }
    bytes = bytes.saturating_add(fields - 1);
    if metadata
        .keys()
        .any(|key| !["cell_id", CALLS, COMPLETE].contains(&key.as_str()))
    {
        bytes.saturating_add(1)
    } else {
        bytes.saturating_add(size(&META) + 4)
    }
}

fn total(items: &[Value]) -> usize {
    items
        .iter()
        .fold(0usize, |n, item| n.saturating_add(observation_bytes(item)))
}

pub(crate) fn has_observations(value: &Value) -> bool {
    value
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| observation_bytes(item) > 0))
}

fn lose(item: &mut Value, losses: &mut Vec<Origin>) {
    losses.push(origin(item));
    if let Some(metadata) = metadata_mut(item) {
        metadata.shift_remove(COMPLETE);
    }
}

fn clear_cells(items: &mut [Value], losses: &[Origin]) {
    let cells: BTreeSet<_> = losses
        .iter()
        .filter_map(|origin| origin.cell.as_deref())
        .collect();
    for item in items {
        if metadata(item)
            .and_then(|m| m.get("cell_id"))
            .and_then(Value::as_str)
            .is_some_and(|cell| cells.contains(cell))
            && let Some(metadata) = metadata_mut(item)
        {
            metadata.shift_remove(COMPLETE);
        }
    }
}

fn truncation(arguments: &Value) -> Option<&Map<String, Value>> {
    let object = arguments.as_object()?;
    if object.len() != 1 {
        return None;
    }
    let marker = object.get(TRUNCATED)?.as_object()?;
    (marker.get("original_bytes")?.as_u64().is_some()
        && marker.get("max_bytes")?.as_u64().is_some())
    .then_some(marker)
}

fn marker(original: usize, maximum: usize, omitted: Option<usize>, name: Option<usize>) -> Value {
    let mut fields = json!({"original_bytes":original,"max_bytes":maximum});
    if let Some(n) = omitted {
        fields["omitted_calls"] = n.into();
    }
    if let Some(n) = name {
        fields["original_name_bytes"] = n.into();
    }
    json!({TRUNCATED:fields})
}

pub(crate) fn prompt(value: &mut Value) -> Vec<Origin> {
    let Some(items) = value.get_mut("input").and_then(Value::as_array_mut) else {
        return Vec::new();
    };
    let mut losses = Vec::new();
    sanitize_calls(items, &mut losses);
    for item in items.iter_mut() {
        let mut damaged = false;
        for call in calls_mut(item).into_iter().flatten() {
            let Some(arguments) = call.get_mut("arguments") else {
                continue;
            };
            if truncation(arguments).is_none() && size(arguments) > ARGUMENT_BYTES {
                *arguments = marker(size(arguments), ARGUMENT_BYTES, None, None);
            }
            damaged |= truncation(arguments).is_some();
        }
        if damaged {
            lose(item, &mut losses);
        }
    }
    clear_cells(items, &losses);
    bound(items, PROMPT_BYTES, false, &mut losses);
    losses
}

pub(crate) fn message(value: &mut Value) -> Vec<Origin> {
    let mut losses = Vec::new();
    if !has_observations(value) {
        return losses;
    }
    sanitize_calls(value["input"].as_array_mut().unwrap(), &mut losses);
    let overage = size(value).saturating_sub(MESSAGE_BYTES);
    if overage == 0 {
        return losses;
    }
    let items = value
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .unwrap();
    let budget = total(items).saturating_sub(overage);
    bound(items, budget, true, &mut losses);
    losses
}

fn sanitize_calls(items: &mut [Value], losses: &mut Vec<Origin>) {
    for item in items.iter_mut() {
        let invalid = metadata(item)
            .and_then(|m| m.get(CALLS))
            .is_some_and(|value| {
                !value.is_null()
                    && value.as_array().is_none_or(|calls| {
                        calls.iter().any(|call| {
                            !call.is_object()
                                || call.get("name").and_then(Value::as_str).is_none()
                                || call.get("arguments").is_none()
                        })
                    })
            });
        if invalid {
            // Malformed optional observations cannot prove a complete inventory. Business
            // output and host metadata survive; the reducer never indexes an untyped scalar.
            lose(item, losses);
            clear_inventory(item);
        }
    }
    clear_cells(items, losses);
}

fn bound(items: &mut [Value], budget: usize, whole: bool, losses: &mut Vec<Origin>) {
    if total(items) <= budget {
        return;
    }
    shedding::generic_results(items, budget, whole);
    if whole && total(items) > budget {
        shedding::sources(items, budget, true);
        shedding::arguments(items, budget, losses);
    }
    shedding::remaining_results(items, budget, whole);
    if total(items) <= budget {
        return;
    }
    for item in items.iter_mut() {
        for call in calls_mut(item).into_iter().flatten() {
            if let Some(call) = call.as_object_mut() {
                call.shift_remove("tool_result_metadata");
            }
        }
    }
    if !whole && total(items) > budget {
        shedding::sources(items, budget, false);
    }
    if total(items) > budget {
        let mut remaining_items = items
            .iter()
            .filter(|item| observation_bytes(item) > 0)
            .count();
        let mut remaining = budget;
        for item in items.iter_mut() {
            let bytes = observation_bytes(item);
            if bytes == 0 {
                continue;
            }
            let share = remaining / remaining_items;
            if bytes > share {
                lose(item, losses);
                bound_item(item, share);
            }
            remaining = remaining.saturating_sub(observation_bytes(item));
            remaining_items -= 1;
        }
    }
    clear_cells(items, losses);
}

fn clear_inventory(item: &mut Value) {
    if let Some(metadata) = metadata_mut(item) {
        for key in ["cell_id", CALLS, COMPLETE] {
            metadata.shift_remove(key);
        }
        if metadata.is_empty() {
            item.as_object_mut().unwrap().shift_remove(META);
        }
    }
}

fn bound_item(item: &mut Value, budget: usize) {
    let Some(existing) = calls(item).filter(|calls| !calls.is_empty()) else {
        clear_inventory(item);
        return;
    };
    let maximum = budget.saturating_sub(observation_bytes(item).saturating_sub(size(existing)));
    let represented = existing.iter().fold(0usize, |sum, call| {
        let omitted = call
            .get("arguments")
            .and_then(truncation)
            .and_then(|m| m.get("omitted_calls"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        sum.saturating_add(1).saturating_add(omitted)
    });
    let calls = calls_mut(item).unwrap();
    calls.truncate(1);
    let call = &mut calls[0];
    let args = call.get("arguments").unwrap_or(&Value::Null);
    let old_marker = truncation(args);
    let original = old_marker
        .and_then(|m| m.get("original_bytes"))
        .and_then(Value::as_u64)
        .map_or_else(|| size(args), |n| n as usize);
    let old_name = old_marker
        .and_then(|m| m.get("original_name_bytes"))
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    call["arguments"] = marker(
        original,
        maximum.min(ARGUMENT_BYTES),
        (represented > 1).then_some(represented - 1),
        old_name,
    );
    if size(calls) > maximum {
        let name = calls[0].get("name").and_then(Value::as_str).unwrap_or("");
        let name_bytes = name.len();
        calls[0]["arguments"] = marker(
            original,
            maximum.min(ARGUMENT_BYTES),
            (represented > 1).then_some(represented - 1),
            Some(old_name.unwrap_or(name_bytes)),
        );
        let excess = size(calls).saturating_sub(maximum);
        if let Some(Value::String(name)) = calls[0].get_mut("name") {
            name.truncate(name.floor_char_boundary(name.len().saturating_sub(excess)));
        }
    }
    if size(calls) > maximum {
        clear_inventory(item);
    }
}

#[cfg(test)]
#[path = "tool_observation_budget_tests.rs"]
mod tests;
