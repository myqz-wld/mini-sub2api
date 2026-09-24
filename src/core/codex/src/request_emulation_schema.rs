use serde_json::Map;
use serde_json::Value;

pub(super) fn canonicalize(value: &mut Value) {
    sanitize(value);
    prune_definitions(value);
    order(value);
}

const FIELDS: &[&str] = &[
    "$ref",
    "type",
    "description",
    "encrypted",
    "enum",
    "items",
    "minItems",
    "properties",
    "required",
    "additionalProperties",
    "anyOf",
    "oneOf",
    "allOf",
    "$defs",
    "definitions",
];

fn order(value: &mut Value) {
    let Some(schema) = value.as_object_mut() else {
        return;
    };
    if let Some(value) = schema.get_mut("items") {
        order(value);
    }
    if let Some(value) = schema
        .get_mut("additionalProperties")
        .filter(|value| value.is_object())
    {
        order(value);
    }
    for name in ["anyOf", "oneOf", "allOf"] {
        if let Some(values) = schema.get_mut(name).and_then(Value::as_array_mut) {
            for value in values {
                order(value);
            }
        }
    }
    for name in ["properties", "$defs", "definitions"] {
        if let Some(entries) = schema.get_mut(name).and_then(Value::as_object_mut) {
            for value in entries.values_mut() {
                order(value);
            }
            let mut sorted = std::mem::take(entries).into_iter().collect::<Vec<_>>();
            sorted.sort_by(|left, right| left.0.cmp(&right.0));
            entries.extend(sorted);
        }
    }
    crate::ignored_fields::retain(schema, FIELDS, "tools[].parameters.schema");
    reorder_preserving(
        schema,
        &[
            "$ref",
            "type",
            "description",
            "encrypted",
            "enum",
            "items",
            "minItems",
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

fn sanitize(value: &mut Value) {
    if value.is_boolean() {
        *value = serde_json::json!({"type":"string"});
    }
    let Some(map) = value.as_object_mut() else {
        return;
    };
    for table in ["properties", "$defs", "definitions"] {
        if let Some(entries) = map.get_mut(table).and_then(Value::as_object_mut) {
            for child in entries.values_mut() {
                sanitize(child);
            }
        } else if table != "properties" && map.contains_key(table) {
            crate::ignored_fields::remove(
                map,
                table,
                "tools[].parameters.schema",
                "invalid_schema_table",
            );
        }
    }
    for key in ["items", "additionalProperties"] {
        if let Some(child) = map.get_mut(key)
            && !(key == "additionalProperties" && child.is_boolean())
        {
            sanitize(child);
        }
    }
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(children) = map.get_mut(key).and_then(Value::as_array_mut) {
            for child in children {
                sanitize(child);
            }
        }
    }
    if let Some(constant) = map.shift_remove("const") {
        // Only a schema position: enum entries (including objects with a const key) are opaque.
        map.insert("enum".into(), Value::Array(vec![constant]));
    }
    let valid = |s: &str| {
        matches!(
            s,
            "string" | "number" | "boolean" | "integer" | "object" | "array" | "null"
        )
    };
    let mut types: Vec<String> = match map.get("type") {
        Some(Value::String(s)) if valid(s) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| valid(s))
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    if types.is_empty()
        && (map.contains_key("$ref")
            || ["anyOf", "oneOf", "allOf"]
                .iter()
                .any(|k| map.contains_key(*k)))
    {
        map.shift_remove("type");
        return;
    }
    if types.is_empty() {
        let inferred = if ["properties", "required", "additionalProperties"]
            .iter()
            .any(|k| map.contains_key(*k))
        {
            Some("object")
        } else if map.contains_key("items") || map.contains_key("prefixItems") {
            Some("array")
        } else if map.contains_key("enum") || map.contains_key("format") {
            Some("string")
        } else if [
            "minimum",
            "maximum",
            "exclusiveMinimum",
            "exclusiveMaximum",
            "multipleOf",
        ]
        .iter()
        .any(|k| map.contains_key(*k))
        {
            Some("number")
        } else {
            None
        };
        let Some(inferred) = inferred else {
            crate::ignored_fields::retain(map, &[], "tools[].parameters.schema");
            return;
        };
        types.push(inferred.into());
    }
    map.insert(
        "type".into(),
        if types.len() == 1 {
            Value::String(types[0].clone())
        } else {
            serde_json::json!(types)
        },
    );
    if types.iter().any(|t| t == "object") {
        map.entry("properties")
            .or_insert_with(|| serde_json::json!({}));
    }
    if types.iter().any(|t| t == "array") {
        map.entry("items")
            .or_insert_with(|| serde_json::json!({"type":"string"}));
    }
    // Optional typed schema fields serialize None by omission.
    map.retain(|_, v| !v.is_null());
}

fn prune_definitions(root: &mut Value) {
    use std::collections::BTreeSet;
    fn collect(value: &Value, include_defs: bool, pending: &mut Vec<(String, String)>) {
        let Some(map) = value.as_object() else { return };
        if let Some(raw) = map
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|s| s.strip_prefix('#'))
            && let Ok(decoded) = urlencoding::decode(raw)
        {
            let mut tokens = decoded.split('/');
            if tokens.next() == Some("")
                && let (Some(table @ ("$defs" | "definitions")), Some(name)) =
                    (tokens.next(), tokens.next())
            {
                pending.push((table.into(), name.replace("~1", "/").replace("~0", "~")));
            }
        }
        for table in ["properties", "$defs", "definitions"] {
            if (table == "properties" || include_defs)
                && let Some(children) = map.get(table).and_then(Value::as_object)
            {
                for child in children.values() {
                    collect(child, include_defs, pending);
                }
            }
        }
        for key in ["items", "additionalProperties"] {
            if let Some(child) = map.get(key) {
                collect(child, include_defs, pending);
            }
        }
        for key in ["anyOf", "oneOf", "allOf"] {
            if let Some(children) = map.get(key).and_then(Value::as_array) {
                for child in children {
                    collect(child, include_defs, pending);
                }
            }
        }
    }
    let mut reachable = BTreeSet::new();
    let mut pending = Vec::new();
    collect(root, false, &mut pending);
    while let Some((table, name)) = pending.pop() {
        if reachable.insert((table.clone(), name.clone()))
            && let Some(child) = root.get(&table).and_then(|t| t.get(&name))
        {
            collect(child, true, &mut pending);
        }
    }
    for table in ["$defs", "definitions"] {
        if let Some(entries) = root.get_mut(table).and_then(Value::as_object_mut) {
            entries.retain(|name, _| {
                let keep = reachable.contains(&(table.into(), name.clone()));
                if !keep {
                    crate::ignored_fields::record(
                        "tools[].parameters.definitions",
                        "entry",
                        "unreachable_definition",
                    );
                }
                keep
            });
            if entries.is_empty() {
                root.as_object_mut().unwrap().shift_remove(table);
            }
        }
    }
}

// Enforce the pinned JsonSchema field types after import lowering. A whitelist alone
// would still transmit malformed required schemas that native deserialization rejects.
pub(super) fn valid_input(value: &Value) -> bool {
    let mut value = value.clone();
    canonicalize(&mut value);
    value.get("type").and_then(Value::as_str) != Some("null") && valid(&value)
}

fn valid(value: &Value) -> bool {
    let Some(schema) = value.as_object() else {
        return false;
    };
    schema.iter().all(|(key, value)| match key.as_str() {
        "$ref" | "description" => value.is_string(),
        "encrypted" => value.is_boolean(),
        "enum" => value.is_array(),
        "type" => {
            value.is_string()
                || value
                    .as_array()
                    .is_some_and(|items| items.iter().all(Value::is_string))
        }
        "minItems" => value.as_u64().is_some_and(|n| usize::try_from(n).is_ok()),
        "required" => value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string)),
        "items" => valid(value),
        "additionalProperties" => value.is_boolean() || valid(value),
        "properties" | "$defs" | "definitions" => value
            .as_object()
            .is_some_and(|entries| entries.values().all(valid)),
        "anyOf" | "oneOf" | "allOf" => value
            .as_array()
            .is_some_and(|items| items.iter().all(valid)),
        _ => false,
    })
}
