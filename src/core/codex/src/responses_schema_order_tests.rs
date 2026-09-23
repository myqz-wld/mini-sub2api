use super::*;
use serde_json::json;

// Byte order from Codex 0.156.0 tools/src/json_schema/types.rs at fe74a774532a.
// Literal expected bytes deliberately do not use the normalizer's ordering table.
const ARRAY: &str = r#"{"type":"array","items":{"type":"string"},"minItems":1}"#;
const COMPOSED_ARRAY: &str = r#"{"type":"array","items":{"type":"string"},"minItems":1,"anyOf":[{"type":"array"}],"oneOf":[{"type":"array"}],"allOf":[{"type":"array"}],"$defs":{"entry":{"type":"string"}}}"#;

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
            "additionalProperties":reverse,"anyOf":[reverse],"$defs":{"entry":reverse}}})]);
        for schema in [
            &tools[0]["parameters"]["properties"]["values"],
            &tools[0]["parameters"]["additionalProperties"],
            &tools[0]["parameters"]["anyOf"][0],
            &tools[0]["parameters"]["$defs"]["entry"],
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
