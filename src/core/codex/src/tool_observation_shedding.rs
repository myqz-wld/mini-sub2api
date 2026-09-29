use super::*;
const RESULT: &str = "tool_result_metadata";
const RESOURCE: &str = "openai/resource_access";

fn result_positions(items: &[Value]) -> Vec<(usize, usize, usize)> {
    let mut result = Vec::new();
    for (i, item) in items.iter().enumerate() {
        for (j, call) in calls(item).into_iter().flatten().enumerate() {
            if let Some(value) = call.get(RESULT) {
                result.push((size(value), i, j));
            }
        }
    }
    result.sort_by_key(|&(bytes, i, j)| (std::cmp::Reverse(bytes), i, j));
    result
}

fn result_mut(items: &mut [Value], i: usize, j: usize) -> &mut Value {
    calls_mut(&mut items[i]).unwrap()[j]
        .get_mut(RESULT)
        .unwrap()
}

fn retain_resource(value: &mut Value) -> bool {
    if let Some(object) = value.as_object_mut()
        && object.contains_key(RESOURCE)
    {
        object.retain(|key, _| key == RESOURCE);
        true
    } else {
        false
    }
}

fn omitted(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        text == "omitted_due_to_size_limit"
            || text
                .strip_prefix("omitted_due_to_size_limit (overage_bytes=")
                .and_then(|s| s.strip_suffix(')'))
                .is_some_and(|s| s.parse::<usize>().is_ok())
    })
}

fn omit(value: &mut Value, overage: usize) {
    let marker = Value::String(format!(
        "omitted_due_to_size_limit (overage_bytes={overage})"
    ));
    if !omitted(value) && size(&marker) < size(value) {
        *value = marker;
    }
}

pub(super) fn generic_results(items: &mut [Value], budget: usize, whole: bool) {
    let positions = result_positions(items);
    let mut bytes = total(items);
    for &(_, i, j) in &positions {
        let overage = bytes.saturating_sub(budget);
        if overage == 0 {
            break;
        }
        let value = result_mut(items, i, j);
        let before = size(value);
        if !retain_resource(value) {
            omit(value, overage);
        }
        bytes = bytes.saturating_sub(before.saturating_sub(size(value)));
    }
    if whole {
        for &(_, i, j) in &positions {
            if bytes <= budget {
                break;
            }
            let value = result_mut(items, i, j);
            let before = size(value);
            if !retain_resource(value) {
                calls_mut(&mut items[i]).unwrap()[j]
                    .as_object_mut()
                    .unwrap()
                    .shift_remove(RESULT);
                bytes = bytes.saturating_sub(before + b",\"tool_result_metadata\":".len());
            }
        }
    }
}

pub(super) fn remaining_results(items: &mut [Value], budget: usize, whole: bool) {
    let positions = result_positions(items);
    let mut bytes = total(items);
    for &(_, i, j) in &positions {
        let overage = bytes.saturating_sub(budget);
        if overage == 0 {
            break;
        }
        let value = result_mut(items, i, j);
        let before = size(value);
        omit(value, overage);
        bytes = bytes.saturating_sub(before.saturating_sub(size(value)));
    }
    let mut positions = result_positions(items);
    positions.sort_by_key(|&(bytes, i, j)| {
        let marker = omitted(calls(&items[i]).unwrap()[j].get(RESULT).unwrap());
        (whole && !marker, std::cmp::Reverse(bytes), !marker, i, j)
    });
    for (value_bytes, i, j) in positions {
        if bytes <= budget {
            break;
        }
        calls_mut(&mut items[i]).unwrap()[j]
            .as_object_mut()
            .unwrap()
            .shift_remove(RESULT);
        bytes = bytes.saturating_sub(value_bytes + b",\"tool_result_metadata\":".len());
    }
}

pub(super) fn sources(items: &mut [Value], budget: usize, whole: bool) {
    let mut bytes = total(items);
    for item in items {
        for call in calls_mut(item).into_iter().flatten() {
            if whole && bytes <= budget {
                return;
            }
            if let Some(call) = call.as_object_mut()
                && let Some(sources) = call.shift_remove("tool_result_sources")
            {
                bytes = bytes.saturating_sub(size(&sources) + b",\"tool_result_sources\":".len());
            }
        }
    }
}

pub(super) fn arguments(items: &mut [Value], budget: usize, losses: &mut Vec<Origin>) {
    for i in 0..items.len() {
        loop {
            let overage = total(items).saturating_sub(budget);
            if overage == 0 {
                return;
            }
            let mut changed = false;
            for call in calls_mut(&mut items[i]).into_iter().flatten() {
                let Some(args) = call.get_mut("arguments") else {
                    continue;
                };
                if truncation(args).is_some() {
                    continue;
                }
                let original = size(args);
                let replacement = marker(
                    original,
                    original.saturating_sub(overage).min(ARGUMENT_BYTES),
                    None,
                    None,
                );
                if size(&replacement) < original {
                    *args = replacement;
                    changed = true;
                    break;
                }
            }
            if !changed {
                break;
            }
            lose(&mut items[i], losses);
            clear_cells(items, losses);
        }
    }
}
