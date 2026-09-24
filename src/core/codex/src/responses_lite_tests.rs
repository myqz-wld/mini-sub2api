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
    assert_eq!(items[2]["id"], "server-id");
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
fn preserves_inline_ids_and_explicit_item_references() {
    let mut request = serde_json::json!({
        "input": [
            {"type":"message","id":"legacy-message","role":"user","content":[]},
            {"type":"item_reference","id":"legacy-reference"}
        ]
    });

    canonicalize_request_items(request.as_object_mut().expect("request"), Some("high"));

    assert_eq!(request["input"][0]["id"], "legacy-message");
    assert_eq!(request["input"][1]["id"], "legacy-reference");
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
            "properties": {},
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
fn filters_non_native_tool_variants_without_traversing_business_enums() {
    let tools = canonicalize_tools(vec![
        serde_json::json!({"type":"file_search"}),
        serde_json::json!({"type":"mcp","headers":{"synthetic":"opaque"}}),
        serde_json::json!({"type":"function","name":"native","parameters":{"type":"string","enum":[{"const":"opaque","name":"opaque"}]}}),
    ]);
    assert_eq!(tools.len(), 1);
    assert_eq!(
        tools[0]["parameters"]["enum"][0],
        serde_json::json!({"const":"opaque","name":"opaque"})
    );
}

#[test]
fn mixed_default_namespaces_keep_native_position_order_and_latest_nonblank_description() {
    let grouped = group_tools(vec![
        serde_json::json!({"type":"namespace","name":"functions","description":"first","tools":[]}),
        serde_json::json!({"type":"tool_search","description":"search"}),
        serde_json::json!({"type":"function","name":"a","parameters":{"const":"opaque"}}),
        serde_json::json!({"type":"namespace","name":"functions","description":"last","tools":[{"type":"custom","name":"b","format":{"type":"text"}}]}),
        serde_json::json!({"type":"namespace","name":"functions","description":"  ","tools":[{"type":"function","name":"a"}]}),
    ]);
    assert_eq!(grouped.len(), 2);
    assert_eq!(grouped[0]["description"], "last");
    assert_eq!(grouped[1]["type"], "tool_search");
    let tools = grouped[0]["tools"].as_array().unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["a", "b", "a"]
    );
    assert_eq!(
        tools[0]["parameters"]["enum"],
        serde_json::json!(["opaque"])
    );
    assert!(
        group_tools(vec![
            serde_json::json!({"type":"namespace","name":"functions","tools":[]})
        ])
        .is_empty()
    );
}
