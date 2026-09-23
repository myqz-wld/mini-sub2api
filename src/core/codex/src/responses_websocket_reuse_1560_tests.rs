use super::*;

const META: &str = "internal_chat_message_metadata_passthrough";

fn tool_output() -> Value {
    json!({"type":"custom_tool_call_output","id":"output-1","call_id":"call-1","output":"synthetic result",
    "internal_chat_message_metadata_passthrough":{"turn_id":"turn-1","executed_tool_calls":[
        {"name":"functions.lookup","arguments":{"key":"value"},"tool_result_metadata":{"revision":1}}
    ]}})
}

#[test]
fn late_tool_result_metadata_or_binding_changes_require_full_history() {
    for change in [
        "added",
        "changed",
        "removed",
        "name",
        "arguments",
        "position",
        "explicit_null",
    ] {
        let mut original = tool_output();
        let mut updated = original.clone();
        match change {
            "added" => {
                original[META]["executed_tool_calls"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("tool_result_metadata");
            }
            "changed" => {
                updated[META]["executed_tool_calls"][0]["tool_result_metadata"] =
                    json!({"revision":2})
            }
            "removed" => {
                updated[META]["executed_tool_calls"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("tool_result_metadata");
            }
            "name" => updated[META]["executed_tool_calls"][0]["name"] = "functions.other".into(),
            "arguments" => {
                updated[META]["executed_tool_calls"][0]["arguments"] = json!({"key":"changed"})
            }
            "position" => updated[META]["executed_tool_calls"]
                .as_array_mut()
                .unwrap()
                .insert(0, json!({"name":"functions.first","arguments":{}})),
            "explicit_null" => {
                updated[META]["executed_tool_calls"][0]["tool_result_metadata"] = Value::Null
            }
            _ => unreachable!(),
        }
        let mut state = state();
        let assistant = message("assistant", "assistant-1");
        establish_public_baseline(
            &mut state,
            &request(vec![original]),
            "response-1",
            std::slice::from_ref(&assistant),
        );
        let current = request(vec![updated, assistant, message("user", "user-2")]);
        let plan = state.plan_public_create(&current);
        assert_eq!(plan.mode, PublicCreateMode::Full, "{change}");
        assert_eq!(
            plan.frame, current,
            "{change}: full request must retain the update"
        );
    }
}

#[test]
fn unchanged_result_metadata_allows_volatile_and_source_evidence_changes() {
    let original = tool_output();
    let mut updated = original.clone();
    updated[META]["turn_id"] = "turn-2".into();
    updated[META]["create_time"] = json!(2);
    updated[META]["tool_calls_complete"] = json!(true);
    updated[META]["executed_tool_calls"][0]["tool_result_sources"] =
        json!([{"type":"url","url":"https://example.test"}]);
    let mut state = state();
    let assistant = message("assistant", "assistant-1");
    establish_public_baseline(
        &mut state,
        &request(vec![original]),
        "response-1",
        std::slice::from_ref(&assistant),
    );
    let next = message("user", "user-2");
    let plan = state.plan_public_create(&request(vec![updated, assistant, next.clone()]));
    assert_eq!(plan.mode, PublicCreateMode::Incremental);
    assert_eq!(plan.frame["previous_response_id"], "response-1");
    assert_eq!(plan.frame["input"], json!([next]));
}

#[test]
fn configuration_updates_are_reusable_history_and_new_controls_stay_in_the_delta() {
    let low = json!({"type":"configuration_update","reasoning":{"effort":"low"}});
    let high = json!({"type":"configuration_update","reasoning":{"effort":"high"}});
    let user = message("user", "user-1");
    let assistant = message("assistant", "assistant-1");
    for replace_history in [false, true] {
        let mut state = state();
        establish_public_baseline(
            &mut state,
            &request(vec![low.clone(), user.clone()]),
            "response-1",
            std::slice::from_ref(&assistant),
        );
        let next = message("user", "user-2");
        let current = request(vec![
            if replace_history {
                high.clone()
            } else {
                low.clone()
            },
            user.clone(),
            assistant.clone(),
            high.clone(),
            next.clone(),
        ]);
        let plan = state.plan_public_create(&current);
        if replace_history {
            assert_eq!(plan.mode, PublicCreateMode::Full);
            assert_eq!(plan.frame, current);
        } else {
            assert_eq!(plan.mode, PublicCreateMode::Incremental);
            assert_eq!(plan.frame["previous_response_id"], "response-1");
            assert_eq!(plan.frame["input"], json!([high, next]));
        }
    }
}
