use serde_json::Map;
use serde_json::Value;

const DOCUMENTED_TOOL_FIELDS: &[&str] = &[
    "type",
    "name",
    "description",
    "strict",
    "allowed_callers",
    "defer_loading",
    "parameters",
    "output_schema",
    "vector_store_ids",
    "filters",
    "max_num_results",
    "ranking_options",
    "display_height",
    "display_width",
    "environment",
    "external_web_access",
    "indexed_web_access",
    "search_context_size",
    "search_content_types",
    "user_location",
    "server_label",
    "allowed_tools",
    "authorization",
    "connector_id",
    "headers",
    "require_approval",
    "server_description",
    "server_url",
    "tunnel_id",
    "container",
    "action",
    "background",
    "input_fidelity",
    "input_image_mask",
    "model",
    "moderation",
    "output_compression",
    "output_format",
    "partial_images",
    "quality",
    "size",
    "format",
    "tools",
    "execution",
];

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
    for name in ["parameters", "output_schema"] {
        if let Some(schema) = object.get_mut(name) {
            super::schema::canonicalize(schema);
        }
    }
    if let Some(format) = object.get_mut("format").and_then(Value::as_object_mut) {
        reorder(format, &["type", "syntax", "definition"]);
    }
    if let Some(filters) = object.get_mut("filters").and_then(Value::as_object_mut) {
        canonicalize_filters(filters);
    }
    if let Some(ranking) = object
        .get_mut("ranking_options")
        .and_then(Value::as_object_mut)
    {
        if let Some(hybrid) = ranking
            .get_mut("hybrid_search")
            .and_then(Value::as_object_mut)
        {
            reorder(hybrid, &["embedding_weight", "text_weight"]);
        }
        reorder(ranking, &["hybrid_search", "ranker", "score_threshold"]);
    }
    if let Some(location) = object
        .get_mut("user_location")
        .and_then(Value::as_object_mut)
    {
        reorder(location, &["type", "country", "region", "city", "timezone"]);
    }
    canonicalize_mcp_controls(object);
    if let Some(container) = object.get_mut("container").and_then(Value::as_object_mut) {
        canonicalize_environment(container);
    }
    if let Some(environment) = object.get_mut("environment").and_then(Value::as_object_mut) {
        canonicalize_environment(environment);
    }
    if let Some(mask) = object
        .get_mut("input_image_mask")
        .and_then(Value::as_object_mut)
    {
        reorder(mask, &["file_id", "image_url"]);
    }
}

fn canonicalize_filters(filters: &mut Map<String, Value>) {
    if let Some(children) = filters.get_mut("filters").and_then(Value::as_array_mut) {
        for child in children {
            if let Some(child) = child.as_object_mut() {
                canonicalize_filters(child);
            }
        }
    }
    reorder(
        filters,
        &["allowed_domains", "key", "type", "value", "filters"],
    );
}

fn canonicalize_mcp_controls(object: &mut Map<String, Value>) {
    if let Some(filter) = object
        .get_mut("allowed_tools")
        .and_then(Value::as_object_mut)
    {
        reorder(filter, &["read_only", "tool_names"]);
    }
    if let Some(tools) = object
        .get_mut("allowed_tools")
        .and_then(Value::as_array_mut)
    {
        for tool in tools {
            if let Some(tool) = tool.as_object_mut() {
                reorder(tool, &["read_only", "tool_names"]);
            }
        }
    }
    if let Some(approval) = object
        .get_mut("require_approval")
        .and_then(Value::as_object_mut)
    {
        for name in ["always", "never"] {
            if let Some(filter) = approval.get_mut(name).and_then(Value::as_object_mut) {
                reorder(filter, &["read_only", "tool_names"]);
            }
        }
        reorder(approval, &["always", "never"]);
    }
}

pub(super) fn canonicalize_environment(environment: &mut Map<String, Value>) {
    if let Some(policy) = environment
        .get_mut("network_policy")
        .and_then(Value::as_object_mut)
    {
        if let Some(secrets) = policy
            .get_mut("domain_secrets")
            .and_then(Value::as_array_mut)
        {
            for secret in secrets {
                if let Some(secret) = secret.as_object_mut() {
                    reorder(secret, &["domain", "name", "value", "secret"]);
                }
            }
        }
        reorder(policy, &["type", "allowed_domains", "domain_secrets"]);
    }
    if let Some(skills) = environment.get_mut("skills").and_then(Value::as_array_mut) {
        for skill in skills {
            if let Some(skill) = skill.as_object_mut() {
                if let Some(source) = skill.get_mut("source").and_then(Value::as_object_mut) {
                    reorder(source, &["data", "media_type", "type"]);
                }
                reorder(
                    skill,
                    &[
                        "skill_id",
                        "type",
                        "version",
                        "description",
                        "name",
                        "source",
                        "path",
                    ],
                );
            }
        }
    }
    reorder(
        environment,
        &[
            "type",
            "file_ids",
            "memory_limit",
            "network_policy",
            "skills",
            "container_id",
        ],
    );
}

fn fields_for_kind(kind: Option<&str>) -> &'static [&'static str] {
    match kind {
        Some("function") => &[
            "type",
            "name",
            "description",
            "strict",
            "allowed_callers",
            "defer_loading",
            "parameters",
            "output_schema",
        ],
        Some("custom") => &[
            "type",
            "name",
            "description",
            "allowed_callers",
            "defer_loading",
            "format",
        ],
        Some("namespace") => &["type", "name", "description", "tools"],
        Some("tool_search") => &["type", "execution", "description", "parameters"],
        Some("web_search" | "web_search_2025_08_26" | "web_search_preview") => &[
            "type",
            "external_web_access",
            "indexed_web_access",
            "filters",
            "user_location",
            "search_context_size",
            "search_content_types",
        ],
        Some("file_search") => &[
            "type",
            "vector_store_ids",
            "filters",
            "max_num_results",
            "ranking_options",
        ],
        Some("computer") => &["type"],
        Some("computer_use_preview") => &["type", "display_width", "display_height", "environment"],
        Some("mcp") => &[
            "type",
            "server_label",
            "allowed_callers",
            "allowed_tools",
            "authorization",
            "connector_id",
            "defer_loading",
            "headers",
            "require_approval",
            "server_description",
            "server_url",
            "tunnel_id",
        ],
        Some("code_interpreter") => &["type", "container", "allowed_callers"],
        Some("programmatic" | "programmatic_tool_calling") => &["type"],
        Some("image_generation") => &[
            "type",
            "action",
            "background",
            "input_fidelity",
            "input_image_mask",
            "model",
            "moderation",
            "output_compression",
            "output_format",
            "partial_images",
            "quality",
            "size",
        ],
        Some("local_shell") => &["type"],
        Some("shell") => &["type", "allowed_callers", "environment"],
        Some("apply_patch") => &["type", "allowed_callers"],
        _ => DOCUMENTED_TOOL_FIELDS,
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
