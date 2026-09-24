use serde_json::Map;
use serde_json::Value;

pub(super) fn canonical_tool(mut tool: Value) -> Value {
    let Some(object) = tool.as_object_mut() else {
        return tool;
    };
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    apply_missing_defaults(object, kind.as_deref());
    canonicalize_nested(object, kind.as_deref());
    let order = fields_for_kind(kind.as_deref());
    reorder(object, order);
    tool
}

fn apply_missing_defaults(object: &mut Map<String, Value>, kind: Option<&str>) {
    if kind == Some("function") {
        object
            .entry("description".to_string())
            .or_insert_with(|| Value::String(String::new()));
        object
            .entry("strict".to_string())
            .or_insert(Value::Bool(false));
        object
            .entry("parameters".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if matches!(kind, Some("custom" | "namespace")) {
        object
            .entry("description".to_string())
            .or_insert_with(|| Value::String(String::new()));
    }
}

fn canonicalize_nested(object: &mut Map<String, Value>, kind: Option<&str>) {
    if kind == Some("namespace")
        && let Some(children) = object.get_mut("tools").and_then(Value::as_array_mut)
    {
        for child in children {
            *child = canonical_tool(std::mem::take(child));
        }
    }
    if let Some(schema) = object.get_mut("parameters") {
        super::schema::canonicalize(schema);
    }
    if let Some(format) = object.get_mut("format").and_then(Value::as_object_mut) {
        reorder(format, &["type", "syntax", "definition"]);
    }
    if kind == Some("web_search")
        && let Some(filters) = object.get_mut("filters").and_then(Value::as_object_mut)
    {
        reorder(filters, &["allowed_domains"]);
    }
    if let Some(location) = object
        .get_mut("user_location")
        .and_then(Value::as_object_mut)
    {
        reorder(location, &["type", "country", "region", "city", "timezone"]);
    }
}

fn fields_for_kind(kind: Option<&str>) -> &'static [&'static str] {
    match kind {
        Some("function") => &[
            "type",
            "name",
            "description",
            "strict",
            "defer_loading",
            "parameters",
        ],
        Some("custom") => &["type", "name", "description", "defer_loading", "format"],
        Some("namespace") => &["type", "name", "description", "tools"],
        Some("tool_search") => &["type", "execution", "description", "parameters"],
        Some("web_search") => &[
            "type",
            "external_web_access",
            "indexed_web_access",
            "filters",
            "user_location",
            "search_context_size",
            "search_content_types",
        ],
        _ => &["type"],
    }
}

fn reorder(object: &mut Map<String, Value>, order: &[&str]) {
    crate::ignored_fields::retain(object, order, "tools[].member");
    let mut existing = std::mem::take(object);
    for name in order {
        if let Some(value) = existing.remove(*name) {
            object.insert((*name).to_string(), value);
        }
    }
}
