use super::*;
use serde_json::json;

// Byte order from Codex 0.159.2 tools/src/json_schema/types.rs at ff6aec96948b.
// Literal expected bytes deliberately do not use the normalizer's ordering table.
const ARRAY: &str = r#"{"type":"array","items":{"type":"string"},"minItems":1}"#;
const COMPOSED_ARRAY: &str = r#"{"type":"array","items":{"type":"string"},"minItems":1,"anyOf":[{"type":"array","items":{"type":"string"}}],"oneOf":[{"type":"array","items":{"type":"string"}}],"allOf":[{"type":"array","items":{"type":"string"}}]}"#;

#[test]
fn min_items_matches_official_order_in_simple_and_composed_schemas() {
    for expected in [ARRAY, COMPOSED_ARRAY] {
        let parsed: Value = serde_json::from_str(expected).unwrap();
        let reverse: Map<_, _> = parsed
            .as_object()
            .unwrap()
            .clone()
            .into_iter()
            .rev()
            .collect();
        let tools = canonicalize_tools(vec![
            json!({"type":"function","name":"lookup","parameters":reverse}),
        ]);
        assert_eq!(
            serde_json::to_string(&tools[0]["parameters"]).unwrap(),
            expected
        );
        let tools = canonicalize_tools(vec![json!({"type":"function","name":"lookup",
            "parameters":{"type":"object","properties":{"values":reverse},
            "additionalProperties":reverse,"anyOf":[reverse]}})]);
        for schema in [
            &tools[0]["parameters"]["properties"]["values"],
            &tools[0]["parameters"]["additionalProperties"],
            &tools[0]["parameters"]["anyOf"][0],
        ] {
            assert_eq!(serde_json::to_string(schema).unwrap(), expected);
        }
    }
}

#[test]
fn lite_prefix_id_uses_official_schema_bytes_and_omits_local_output_schema() {
    let schema: Value = serde_json::from_str(COMPOSED_ARRAY).unwrap();
    let tools = group_tools(vec![
        json!({"type":"function","name":"lookup","description":"",
        "strict":false,"parameters":schema,"output_schema":{"type":"string"}}),
    ]);
    let expected = format!(
        r#"[{{"type":"namespace","name":"functions","description":"","tools":[{{"type":"function","name":"lookup","description":"","strict":false,"parameters":{COMPOSED_ARRAY}}}]}}]"#
    );
    assert_eq!(serde_json::to_string(&tools).unwrap(), expected);
    let mut body = json!({"input":[{"type":"additional_tools","tools":tools}]});
    crate::lite_prefix_identity::apply_generated(
        body.as_object_mut().unwrap(),
        "thread-test",
        &[0],
    )
    .unwrap();
    let namespace = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"thread-test");
    assert_eq!(
        body["input"][0]["id"],
        format!("at_{}", Uuid::new_v5(&namespace, expected.as_bytes()))
    );
}

#[test]
fn native_typed_parameters_preserve_type_free_nodes_and_definitions() {
    // The typed catalog producer accepts primitive null; the raw MCP importer
    // rejects it after lowering. Do not apply that importer-only check twice.
    assert!(valid_tool_parameters(&json!({"type":"null"})));
    assert!(!valid_tool_parameters(&json!({"type":"null","const":null})));
    let parameters = json!({"type":"object","properties":{
        "budget":{"enum":[128,256]}, "context":{"description":"opaque values"},
        "optional":{"type":null,"description":"nullable declaration"}
    }, "$defs":{"Unused":{"type":"string"}}, "additionalProperties":false});
    for tools in [
        canonicalize_tools(vec![
            json!({"type":"function","name":"wait","parameters":parameters}),
        ]),
        group_tools(vec![
            json!({"type":"function","name":"wait","parameters":parameters}),
        ]),
    ] {
        let tool = if tools[0]["type"] == "namespace" {
            &tools[0]["tools"][0]
        } else {
            &tools[0]
        };
        let schema = &tool["parameters"];
        assert_eq!(schema["properties"]["budget"], json!({"enum":[128,256]}));
        assert_eq!(
            schema["properties"]["context"],
            parameters["properties"]["context"]
        );
        assert!(schema["properties"]["optional"].get("type").is_none());
        assert_eq!(schema["$defs"], parameters["$defs"]);
    }
    // Raw import keywords still use the existing native MCP lowering policy.
    let raw = canonicalize_tools(vec![json!({"type":"function","name":"raw",
        "parameters":{"type":"object","properties":{"mode":{"const":"synthetic"}}}})]);
    assert_eq!(
        raw[0]["parameters"]["properties"]["mode"],
        json!({"type":"string","enum":["synthetic"]})
    );
}

#[test]
fn native_search_action_preserves_order_and_omits_unknown_or_null_members() {
    for (action, expected) in [
        (
            json!({"queries":["synthetic"],"query":"synthetic","sources":[],"type":"search"}),
            r#"{"type":"search","query":"synthetic","queries":["synthetic"]}"#,
        ),
        (
            json!({"type":"search","query":null,"queries":["synthetic"]}),
            r#"{"type":"search","queries":["synthetic"]}"#,
        ),
        (
            json!({"type":"open_page","url":null}),
            r#"{"type":"open_page"}"#,
        ),
    ] {
        let mut items = vec![json!({"type":"web_search_call","action":action})];
        assign_missing_item_ids(&mut items);
        assert_eq!(
            serde_json::to_string(&items[0]["action"]).unwrap(),
            expected
        );
    }
}
