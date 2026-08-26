use serde_json::Map;
use serde_json::Value;

pub(super) fn canonicalize(value: &mut Value) {
    let Some(schema) = value.as_object_mut() else {
        return;
    };
    if let Some(value) = schema.get_mut("items") {
        canonicalize(value);
    }
    if let Some(value) = schema
        .get_mut("additionalProperties")
        .filter(|value| value.is_object())
    {
        canonicalize(value);
    }
    for name in ["anyOf", "oneOf", "allOf"] {
        if let Some(values) = schema.get_mut(name).and_then(Value::as_array_mut) {
            for value in values {
                canonicalize(value);
            }
        }
    }
    for name in ["properties", "$defs", "definitions"] {
        if let Some(entries) = schema.get_mut(name).and_then(Value::as_object_mut) {
            for value in entries.values_mut() {
                canonicalize(value);
            }
            let mut sorted = std::mem::take(entries).into_iter().collect::<Vec<_>>();
            sorted.sort_by(|left, right| left.0.cmp(&right.0));
            entries.extend(sorted);
        }
    }
    reorder_preserving(
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

fn reorder_preserving(object: &mut Map<String, Value>, order: &[&str]) {
    let mut existing = std::mem::take(object);
    for name in order {
        if let Some(value) = existing.remove(*name) {
            object.insert((*name).to_string(), value);
        }
    }
    object.extend(existing);
}
