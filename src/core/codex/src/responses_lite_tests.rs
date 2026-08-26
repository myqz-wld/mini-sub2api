use super::*;

#[test]
fn groups_default_tools_and_matches_item_identity_metadata() {
    let tools = group_tools(vec![
        serde_json::json!({"type":"tool_search","description":"search"}),
        serde_json::json!({"type":"function","name":"lookup","description":"","strict":false,"parameters":{}}),
        serde_json::json!({"type":"custom","name":"exec","description":"","format":{"type":"grammar","syntax":"lark","definition":""}}),
    ]);
    assert_eq!(tools[0]["type"], "tool_search");
    assert_eq!(tools[1]["type"], "namespace");
    assert_eq!(tools[1]["name"], DEFAULT_NAMESPACE);
    assert_eq!(tools[1]["tools"].as_array().map(Vec::len), Some(2));

    let mut items = vec![
        serde_json::json!({"type":"message","role":"user","content":[]}),
        serde_json::json!({"type":"message","role":"assistant","content":[]}),
        serde_json::json!({"type":"function_call","id":"server-id","name":"lookup","arguments":"{}","call_id":"call"}),
        serde_json::json!({"type":"function_call_output","call_id":"call","output":"done"}),
        serde_json::json!({"type":"reasoning","summary":[]}),
        serde_json::json!({"type":"tool_search_call","status":null,"execution":"server","arguments":{}}),
    ];
    let synthesized = assign_missing_item_ids(&mut items);
    assert_eq!(synthesized.len(), 5);
    let id = items[0]["id"].as_str().expect("message id");
    assert!(id.starts_with("msg_"));
    assert_eq!(
        Uuid::parse_str(&id[4..]).expect("UUID").get_version_num(),
        7
    );
    assert_eq!(items[2]["id"], "server-id");

    let mut request = serde_json::json!({
        "input": items,
        "client_metadata": {"turn_id": "turn-test"}
    });
    canonicalize_request_items(request.as_object_mut().expect("request"), Some("high"));
    let items = request["input"].as_array().expect("items");
    assert!(items[2].get("id").is_none());
    for item in items {
        assert_eq!(
            item["internal_chat_message_metadata_passthrough"]["turn_id"],
            "turn-test"
        );
    }
    assert!(items[0]["internal_chat_message_metadata_passthrough"]["create_time"].is_number());
    assert!(
        items[1]["internal_chat_message_metadata_passthrough"]
            .get("create_time")
            .is_none()
    );
    assert!(items[3]["internal_chat_message_metadata_passthrough"]["create_time"].is_number());
    assert!(
        items[4]["internal_chat_message_metadata_passthrough"]
            .get("create_time")
            .is_none()
    );
    assert!(items[4]["encrypted_content"].is_null());
    assert!(items[5]["call_id"].is_null());
    assert!(items[5]["status"].is_null());
}

#[test]
fn canonicalizes_nested_additional_properties_schema() {
    let tools = canonicalize_tools(vec![serde_json::json!({
        "type": "function",
        "name": "lookup",
        "parameters": {
            "additionalProperties": {
                "description": "nested",
                "type": "string"
            },
            "type": "object"
        }
    })]);

    assert_eq!(
        tools[0]["parameters"],
        serde_json::json!({
            "type": "object",
            "additionalProperties": {
                "type": "string",
                "description": "nested"
            }
        })
    );
}

#[test]
fn canonicalizes_legacy_compaction_and_action_optionals() {
    let mut request = serde_json::json!({
        "input": [
            {"type":"compaction_summary","encrypted_content":"opaque"},
            {"type":"local_shell_call","status":"completed","action":{"type":"exec","command":["pwd"]}},
            {"type":"web_search_call","action":{"type":"search","query":null,"queries":["codex"]}}
        ]
    });
    canonicalize_request_items(request.as_object_mut().expect("request"), Some("high"));
    let items = request["input"].as_array().expect("items");

    assert_eq!(items[0]["type"], "compaction");
    for name in ["timeout_ms", "working_directory", "env", "user"] {
        assert!(items[1]["action"][name].is_null());
    }
    assert!(items[2]["action"]["query"].is_null());
    assert_eq!(items[2]["action"]["queries"][0], "codex");
}

#[test]
fn canonicalizes_codex_output_schema_controls() {
    let mut request = serde_json::json!({
        "text": {"format": {"type":"json_schema","schema":{"type":"string"}}}
    });
    canonicalize_request_items(request.as_object_mut().expect("request"), Some("high"));

    assert_eq!(request["text"]["format"]["strict"], true);
    assert_eq!(request["text"]["format"]["name"], "codex_output_schema");

    let mut explicit = serde_json::json!({
        "text": {"format": {
            "type":"json_schema",
            "strict":false,
            "name":"caller_schema",
            "schema":{"type":"object","x-schema-extension":{"opaque":true}}
        }}
    });
    canonicalize_request_items(explicit.as_object_mut().expect("request"), Some("high"));
    assert_eq!(explicit["text"]["format"]["strict"], false);
    assert_eq!(explicit["text"]["format"]["name"], "caller_schema");
    assert_eq!(
        explicit["text"]["format"]["schema"]["x-schema-extension"]["opaque"],
        true
    );
}

#[test]
fn filters_documented_tool_variants_without_touching_free_form_containers() {
    let tools = canonicalize_tools(vec![
        serde_json::json!({
            "type":"file_search",
            "vector_store_ids":["vs_test"],
            "filters":{"key":"kind","type":"eq","value":"doc","unsupported_filter":true},
            "ranking_options":{
                "hybrid_search":{"embedding_weight":0.5,"text_weight":0.5,"unsupported_hybrid":true},
                "ranker":"auto","score_threshold":0.1,"unsupported_ranking":true
            },
            "unsupported_tool":true
        }),
        serde_json::json!({
            "type":"mcp","server_label":"docs","server_url":"https://example.test/mcp",
            "headers":{"x-free-form-header":"preserved"},
            "allowed_tools":[{"read_only":true,"tool_names":["lookup"],"unsupported_filter":true}],
            "require_approval":{"always":{"read_only":true,"unsupported_filter":true},"unsupported_approval":true},
            "unsupported_tool":true
        }),
        serde_json::json!({
            "type":"code_interpreter",
            "container":{
                "type":"auto","file_ids":["file_test"],
                "network_policy":{"type":"allowlist","allowed_domains":["example.test"],"unsupported_policy":true},
                "unsupported_container":true
            },
            "unsupported_tool":true
        }),
        serde_json::json!({
            "type":"image_generation","action":"edit",
            "input_image_mask":{"file_id":"file_mask","image_url":"data:image/png;base64,AA==","unsupported_mask":true},
            "quality":"high","unsupported_tool":true
        }),
        serde_json::json!({
            "type":"shell","allowed_callers":["direct"],
            "environment":{
                "type":"container_auto","memory_limit":"1g",
                "skills":[{"type":"inline","name":"skill","description":"test","source":{
                    "type":"base64","media_type":"application/zip","data":"AA==","unsupported_source":true
                },"unsupported_skill":true}],
                "unsupported_environment":true
            },
            "unsupported_tool":true
        }),
        serde_json::json!({
            "type":"future_tool","name":"documented-field-name","unsupported_tool":true
        }),
    ]);

    for tool in &tools {
        assert!(tool.get("unsupported_tool").is_none());
    }
    assert!(tools[0]["filters"].get("unsupported_filter").is_none());
    assert!(
        tools[0]["ranking_options"]
            .get("unsupported_ranking")
            .is_none()
    );
    assert!(
        tools[0]["ranking_options"]["hybrid_search"]
            .get("unsupported_hybrid")
            .is_none()
    );
    assert_eq!(tools[1]["headers"]["x-free-form-header"], "preserved");
    assert!(
        tools[1]["allowed_tools"][0]
            .get("unsupported_filter")
            .is_none()
    );
    assert!(
        tools[1]["require_approval"]
            .get("unsupported_approval")
            .is_none()
    );
    assert!(tools[2]["container"].get("unsupported_container").is_none());
    assert!(
        tools[2]["container"]["network_policy"]
            .get("unsupported_policy")
            .is_none()
    );
    assert!(
        tools[3]["input_image_mask"]
            .get("unsupported_mask")
            .is_none()
    );
    assert!(
        tools[4]["environment"]["skills"][0]
            .get("unsupported_skill")
            .is_none()
    );
    assert!(
        tools[4]["environment"]["skills"][0]["source"]
            .get("unsupported_source")
            .is_none()
    );
    assert_eq!(tools[5]["name"], "documented-field-name");
}
