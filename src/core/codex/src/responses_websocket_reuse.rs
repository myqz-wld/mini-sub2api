use crate::responses_websocket_projection::equivalent_items;
use crate::responses_websocket_projection::project_properties;
use crate::responses_websocket_projection::reusable_item;
use serde_json::Map;
use serde_json::Value;
use std::collections::HashSet;

const EXPLICIT_STATE_CARRIERS: &[&str] = &[
    "previous_response_id",
    "conversation",
    "generate",
    "stream_id",
];

pub(crate) struct RequestSnapshot {
    pub(crate) properties: Map<String, Value>,
    pub(crate) input: Vec<Value>,
    comparison_input: Vec<Value>,
}

pub(crate) struct ReuseBaseline {
    pub(crate) request: RequestSnapshot,
    pub(crate) response_id: String,
    pub(crate) output: Vec<Value>,
}

pub(crate) fn has_explicit_state_carrier(request: &Value) -> bool {
    request.as_object().is_some_and(|object| {
        EXPLICIT_STATE_CARRIERS
            .iter()
            .any(|field| object.contains_key(*field))
    })
}

pub(crate) fn request_snapshot(
    request: &Value,
    synthesized_item_ids: &[String],
) -> Option<RequestSnapshot> {
    let object = request.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("response.create") {
        return None;
    }
    let input = object.get("input")?.as_array()?.clone();
    if !input.iter().all(reusable_item) {
        return None;
    }
    let synthesized = synthesized_item_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let comparison_input = input
        .iter()
        .cloned()
        .map(|mut item| {
            if let Some(object) = item.as_object_mut()
                && object
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| synthesized.contains(id))
            {
                object.remove("id");
            }
            item
        })
        .collect();
    Some(RequestSnapshot {
        properties: project_properties(object),
        input,
        comparison_input,
    })
}

pub(crate) fn lite_prewarm_prefix(input: &[Value]) -> Option<Vec<Value>> {
    let first = input.first()?;
    if first
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        != Some("additional_tools")
    {
        return None;
    }
    let prefix_len = 1 + input[1..]
        .iter()
        .take_while(|item| {
            item.as_object().is_some_and(|object| {
                object.get("type").and_then(Value::as_str) == Some("message")
                    && object.get("role").and_then(Value::as_str) == Some("developer")
            })
        })
        .count();
    Some(input[..prefix_len].to_vec())
}

pub(crate) fn incremental_input(
    baseline: &ReuseBaseline,
    current: &RequestSnapshot,
) -> Option<Vec<Value>> {
    if baseline.request.properties != current.properties || baseline.response_id.is_empty() {
        return None;
    }
    let prefix_len = baseline
        .request
        .comparison_input
        .len()
        .checked_add(baseline.output.len())?;
    let (comparison_prefix, _) = current.comparison_input.split_at_checked(prefix_len)?;
    let expected = baseline
        .request
        .comparison_input
        .iter()
        .chain(&baseline.output)
        .cloned()
        .collect::<Vec<_>>();
    if !equivalent_items(&expected, comparison_prefix) {
        return None;
    }
    let (_, delta) = current.input.split_at_checked(prefix_len)?;
    Some(delta.to_vec())
}
