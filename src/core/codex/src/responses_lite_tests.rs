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
    assign_missing_item_ids(&mut items);
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
    canonicalize_request_items(request.as_object_mut().expect("request"));
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
    assert!(items[5].get("status").is_none());
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
    canonicalize_request_items(request.as_object_mut().expect("request"));
    let items = request["input"].as_array().expect("items");

    assert_eq!(items[0]["type"], "compaction");
    for name in ["timeout_ms", "working_directory", "env", "user"] {
        assert!(items[1]["action"][name].is_null());
    }
    assert!(items[2]["action"].get("query").is_none());
    assert_eq!(items[2]["action"]["queries"][0], "codex");
}

#[test]
fn canonicalizes_codex_output_schema_controls() {
    let mut request = serde_json::json!({
        "text": {"format": {"type":"json_schema","schema":{"type":"string"}}}
    });
    canonicalize_request_items(request.as_object_mut().expect("request"));

    assert_eq!(request["text"]["format"]["strict"], true);
    assert_eq!(request["text"]["format"]["name"], "codex_output_schema");
}
