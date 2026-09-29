use super::*;

fn output(id: &str, cell: &str, arguments: Value, result: Value) -> Value {
    json!({"type":"function_call_output","id":id,"call_id":id,"output":"business-output",
        META:{"turn_id":"turn","cell_id":cell,CALLS:[{"name":"tools.probe","arguments":arguments,
        "tool_result_sources":[{"type":"url","id":"synthetic-source"}],"tool_result_metadata":result}],
        COMPLETE:true}})
}

#[test]
fn observation_byte_count_includes_wrappers_escaped_cells_and_commas() {
    for base in [false, true] {
        for fields in [1, 2, 3] {
            let mut metadata = json!({"cell_id":"cell\"🦀"});
            if fields > 1 {
                metadata[CALLS] = json!([]);
            }
            if fields > 2 {
                metadata[COMPLETE] = true.into();
            }
            if base {
                metadata["turn_id"] = "turn".into();
            }
            let item = json!({"type":"message",META:metadata});
            let mut stripped = item.clone();
            for key in ["cell_id", CALLS, COMPLETE] {
                stripped[META].as_object_mut().unwrap().shift_remove(key);
            }
            if !base {
                stripped.as_object_mut().unwrap().shift_remove(META);
            }
            assert_eq!(observation_bytes(&item), size(&item) - size(&stripped));
        }
    }
}

#[test]
fn prompt_bounds_arguments_and_invalidates_every_output_in_the_cell() {
    let mut request = json!({"input":[output("one","same",json!({"opaque":"x".repeat(ARGUMENT_BYTES)}),json!({})),
        output("two","same",json!({}),json!({})), output("other","other",json!({}),json!({}))]});
    let losses = prompt(&mut request);
    assert!(!losses.is_empty());
    let items = request["input"].as_array().unwrap();
    assert!(
        items[0][META][CALLS][0]["arguments"]
            .get(TRUNCATED)
            .is_some()
    );
    for item in &items[..2] {
        assert!(item[META].get(COMPLETE).is_none());
    }
    assert_eq!(items[2][META][COMPLETE], true);
    for item in items {
        assert_eq!(item["output"], "business-output");
    }
}

#[test]
fn prompt_sheds_large_generic_results_before_small_results_or_inventory() {
    let resource =
        json!({"openai/resource_access":[{"id":"opaque"}],"large":"x".repeat(PROMPT_BYTES)});
    let mut request = json!({"input":[output("one","same",json!({}),resource),
        output("two","same",json!({}),json!({"small":true}))]});
    assert!(prompt(&mut request).is_empty());
    let items = request["input"].as_array().unwrap();
    assert!(total(items) <= PROMPT_BYTES);
    assert_eq!(
        items[0][META][CALLS][0]["tool_result_metadata"],
        json!({"openai/resource_access":[{"id":"opaque"}]})
    );
    assert_eq!(
        items[1][META][CALLS][0]["tool_result_metadata"],
        json!({"small":true})
    );
    assert_eq!(items[0][META][COMPLETE], true);
}

#[test]
fn message_uses_actual_envelope_and_preserves_soft_limit_business_content() {
    let ordinary = "x".repeat(MESSAGE_BYTES - 2048);
    let mut request = json!({"instructions":ordinary,"input":[
        output("one","same",json!({"large":"y".repeat(4096)}),json!({"openai/resource_access":"keep"})),
        output("two","same",json!({}),json!({}))]});
    assert!(prompt(&mut request).is_empty());
    let baseline = request.clone();
    let losses = message(&mut request);
    assert!(!losses.is_empty());
    assert!(size(&request) <= MESSAGE_BYTES);
    assert_eq!(request["instructions"], baseline["instructions"]);
    assert!(
        baseline["input"][0][META][CALLS][0]["arguments"]
            .get(TRUNCATED)
            .is_none()
    );
    assert_eq!(
        request["input"][0][META][CALLS][0]["tool_result_metadata"],
        json!({"openai/resource_access":"keep"})
    );
    assert!(request["input"][1][META].get(COMPLETE).is_none());
    request["instructions"] = "x".repeat(MESSAGE_BYTES + 1).into();
    message(&mut request);
    assert!(size(&request) > MESSAGE_BYTES);
    assert_eq!(
        request["instructions"].as_str().unwrap().len(),
        MESSAGE_BYTES + 1
    );
    assert_eq!(request["input"][0]["output"], "business-output");
}

#[test]
fn tiny_budgets_remove_only_observations_and_names_remain_utf8() {
    for budget in [0, 16, 128, 512] {
        let mut item = output("one", "same", json!({}), json!({}));
        item[META][CALLS][0]["name"] = "🦀".repeat(1024).into();
        let mut losses = Vec::new();
        let mut items = vec![item];
        bound(&mut items, budget, true, &mut losses);
        assert!(total(&items) <= budget);
        assert_eq!(items[0]["output"], "business-output");
        assert_eq!(items[0][META]["turn_id"], "turn");
    }
}

#[test]
fn malformed_optional_calls_are_removed_without_panicking_or_losing_business_output() {
    for invalid in [
        json!(["x".repeat(PROMPT_BYTES + 1)]),
        json!([null]),
        json!([17]),
        json!([[]]),
        json!([{"arguments":{}}]),
        json!([{"name":"probe"}]),
        json!({"invalid":true}),
    ] {
        for reducer in [prompt, message] {
            let mut item = output("one", "same", json!({}), json!({}));
            item[META][CALLS] = invalid.clone();
            let mut request = json!({"instructions":"x".repeat(MESSAGE_BYTES),"input":[item]});
            assert!(!reducer(&mut request).is_empty());
            assert!(request["input"][0][META].get(CALLS).is_none());
            assert!(request["input"][0][META].get(COMPLETE).is_none());
            assert_eq!(request["input"][0]["output"], "business-output");
            assert_eq!(request["input"][0][META]["turn_id"], "turn");
        }
    }
}
