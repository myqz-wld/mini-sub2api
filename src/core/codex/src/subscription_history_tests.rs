use super::*;

#[path = "subscription_history_config_tests.rs"]
mod config_tests;

fn assistant() -> Value {
    json!({"type":"message","id":"msg_history","status":"completed","role":"assistant",
        "content":[{"type":"output_text","text":"answer","annotations":[],"logprobs":[]}]})
}

fn caller_copy(mut item: Value) -> Value {
    let object = item.as_object_mut().unwrap();
    object.remove("id");
    object.remove("status");
    if let Some(content) = object.get_mut("content").and_then(Value::as_array_mut) {
        for part in content {
            part.as_object_mut().unwrap().remove("annotations");
            part.as_object_mut().unwrap().remove("logprobs");
        }
    }
    item
}

fn plan(
    store: &RequestStateStore,
    body: &Value,
    key: &str,
) -> Result<crate::subscription_prepare::ContextPlan, StatefulPrepareError> {
    let object = body.as_object().unwrap();
    let evidence = Evidence::read(object, &HeaderMap::new(), EmulationTransport::Http)?;
    store.contexts.plan(
        ContextStore::scope_key(NAMESPACE, key),
        object,
        evidence,
        None,
        None,
    )
}

#[tokio::test]
async fn baseless_anonymous_history_and_explicit_reconstruction_stay_baseless() {
    for model in ["gpt-5.4", "gpt-5.6-sol"] {
        let (_temp, store) = store();
        let first = json!({"model":model,"input":[input("first")],"tools":[]});
        let prepared = prepare(&store, first.clone()).await.unwrap();
        let identity = prepared.resolved_identity.as_ref().unwrap().clone();
        let emitted: Value = serde_json::from_slice(&prepared.body).unwrap();
        let prefix = usize::from(model == "gpt-5.6-sol");
        assert_eq!(emitted["input"].as_array().unwrap().len(), 1 + prefix);
        assert!(emitted["input"][prefix]["role"] == "user");
        let response = publish(
            &store,
            prepared,
            "resp_baseless_first",
            json!([assistant()]),
        )
        .await;
        let mut next = first.clone();
        next["instructions"] = Value::Null;
        next["input"] = json!([
            input("first"),
            caller_copy(response["output"][0].clone()),
            input("second")
        ]);
        assert!(plan(&store, &next, KEY).unwrap().baseline.is_some());
        let prepared = prepare(&store, next).await.unwrap();
        assert_eq!(
            prepared.resolved_identity.as_ref().unwrap().session_id,
            identity.session_id
        );
        let response = publish(&store, prepared, "resp_baseless_second", json!([])).await;
        let mut delta = first;
        delta["previous_response_id"] = response["id"].clone();
        delta["input"] = json!([input("third")]);
        let prepared = prepare(&store, delta).await.unwrap();
        let full: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert!(full.get("previous_response_id").is_none());
        assert!(
            full["input"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["type"] != "message" || item["role"] != "developer")
        );
        assert_eq!(full["input"].as_array().unwrap().len(), 4 + prefix);
        if prefix == 1 {
            assert!(full.get("instructions").is_none());
            assert!(full["input"][0]["id"] == emitted["input"][0]["id"]);
        } else {
            assert!(full.get("instructions").is_none());
        }
    }
}

#[tokio::test]
async fn simplified_text_history_keeps_session_and_materializes_the_callers_actual_input() {
    for model in ["gpt-5.4", "gpt-6-astra"] {
        let (_temp, store) = store();
        let mut first = request(json!([input("first")]));
        first["model"] = model.into();
        let prepared = prepare(&store, first.clone()).await.unwrap();
        let identity = prepared.resolved_identity.as_ref().unwrap().clone();
        let response = publish(&store, prepared, "resp_text_history", json!([assistant()])).await;
        let simplified = caller_copy(response["output"][0].clone());
        let mut next = first.clone();
        next["input"] = json!([input("first"), simplified, input("second")]);
        assert!(plan(&store, &next, KEY).unwrap().baseline.is_some());
        let next = prepare(&store, next).await.unwrap();
        let next_identity = next.resolved_identity.as_ref().unwrap();
        assert_eq!(next_identity.session_id, identity.session_id);
        assert_eq!(next_identity.thread_id, identity.thread_id);
        assert_ne!(next_identity.turn_id, identity.turn_id);
        let second = publish(&store, next, "resp_materialized", json!([])).await;
        {
            let inner = store.contexts.inner.lock().unwrap();
            let record = &inner.scopes[&ContextStore::scope_key(NAMESPACE, KEY)].records
                [second["id"].as_str().unwrap()];
            assert!(
                record.history.as_ref().unwrap().values()[1] == simplified,
                "matching must not replace caller input with the fuller saved output"
            );
        }
        first["previous_response_id"] = second["id"].clone();
        first["input"] = json!([input("third")]);
        let expanded = prepare(&store, first).await.unwrap();
        let body: Value = serde_json::from_slice(&expanded.body).unwrap();
        let output = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["role"] == "assistant")
            .unwrap();
        assert!(output.get("id").is_none());
        assert!(output["content"][0].get("annotations").is_none());
        assert!(output["content"][0].get("logprobs").is_none());
        assert!(body.get("previous_response_id").is_none());
    }
}

#[tokio::test]
async fn call_anchored_history_retains_tool_turn_and_rejects_a_consumed_result() {
    for model in ["gpt-5.4", "gpt-6-astra"] {
        for (kind, result_kind) in [
            ("function_call", "function_call_output"),
            ("custom_tool_call", "custom_tool_call_output"),
        ] {
            let (_temp, store) = store();
            let mut first = request(json!([input("use tool")]));
            first["model"] = model.into();
            let prepared = prepare(&store, first.clone()).await.unwrap();
            let identity = prepared.resolved_identity.as_ref().unwrap().clone();
            let mut call = json!({"type":kind,"id":"item_tool","call_id":"call_tool",
                "name":"tool","status":"completed"});
            call[if kind == "function_call" {
                "arguments"
            } else {
                "input"
            }] = "{}".into();
            let response = publish(&store, prepared, "resp_tool_history", json!([call])).await;
            let simplified = caller_copy(response["output"][0].clone());
            let result =
                json!({"type":result_kind,"call_id":simplified["call_id"],"output":"tool result"});
            let mut next = first.clone();
            next["input"] = json!([input("use tool"), simplified, result]);
            assert!(plan(&store, &next, KEY).unwrap().baseline.is_some());
            let next = prepare(&store, next).await.unwrap();
            let next_identity = next.resolved_identity.as_ref().unwrap();
            assert_eq!(next_identity.session_id, identity.session_id);
            assert_eq!(next_identity.thread_id, identity.thread_id);
            assert_eq!(next_identity.turn_id, identity.turn_id);
            let completed = publish(&store, next, "resp_tool_consumed", json!([])).await;
            first["previous_response_id"] = completed["id"].clone();
            first["input"] = json!([result]);
            assert!(matches!(
                prepare(&store, first).await,
                Err(StatefulPrepareError::InvalidRequest)
            ));
        }
    }
}

#[tokio::test]
async fn simplified_history_still_requires_content_ids_and_scope_to_agree() {
    for variant in ["empty", "nonempty-annotations", "nonempty-logprobs"] {
        let (_temp, store) = store();
        let first = prepare(&store, request(json!([input("first")])))
            .await
            .unwrap();
        let mut output = assistant();
        if variant == "nonempty-annotations" {
            output["content"][0]["annotations"] =
                json!([{"type":"url_citation","url":"https://example.invalid"}]);
        } else if variant == "nonempty-logprobs" {
            output["content"][0]["logprobs"] = json!([{"token":"answer","logprob":-0.1}]);
        }
        let response = publish(&store, first, "resp_decorated", json!([output])).await;
        let simplified = caller_copy(response["output"][0].clone());
        let mut next = request(json!([input("first"), simplified, input("next")]));
        assert_eq!(
            plan(&store, &next, KEY).unwrap().baseline.is_some(),
            variant == "empty"
        );
        assert!(plan(&store, &next, "other-key").unwrap().baseline.is_none());
        next["client_metadata"] = json!({"session_id":"another-explicit-session"});
        assert!(plan(&store, &next, KEY).unwrap().baseline.is_none());
        next.as_object_mut().unwrap().remove("client_metadata");
        next["input"][1]["id"] = "conflicting-explicit-id".into();
        assert!(plan(&store, &next, KEY).unwrap().baseline.is_none());
    }
}

#[tokio::test]
async fn tool_history_requires_exact_call_semantics_and_any_explicit_item_id() {
    let (_temp, store) = store();
    let first = prepare(&store, request(json!([input("first")])))
        .await
        .unwrap();
    let call = json!({"type":"function_call","id":"fc_history","call_id":"call_history",
        "name":"tool","arguments":"{\"n\":1}","status":"completed"});
    let response = publish(&store, first, "resp_call_fields", json!([call])).await;
    let minimal = caller_copy(response["output"][0].clone());
    for (field, value) in [
        ("id", "other-id"),
        ("call_id", "other-call"),
        ("name", "other-tool"),
        ("arguments", "{\"n\":2}"),
        ("status", "in_progress"),
    ] {
        let mut changed = minimal.clone();
        changed[field] = value.into();
        let next = request(json!([input("first"), changed, input("next")]));
        assert!(
            plan(&store, &next, KEY).unwrap().baseline.is_none(),
            "changed {field} matched"
        );
    }
    let next = request(json!([input("first"), minimal,
        {"type":"function_call_output","call_id":"unknown-call","output":"value"}]));
    assert!(matches!(
        prepare(&store, next).await,
        Err(StatefulPrepareError::StateUnavailable)
    ));
}

#[tokio::test]
async fn history_compatibility_does_not_weaken_provider_output_consistency() {
    for tool in [false, true] {
        let (_temp, store) = store();
        let first = prepare(&store, request(json!([input("first")])))
            .await
            .unwrap();
        let state = ResponseStateContext::new(
            OWNER,
            NAMESPACE,
            KEY,
            &store,
            first.resolved_identity.as_ref(),
            None,
        )
        .with_operation(first.operation);
        let output = if tool {
            json!({"type":"function_call","id":"fc_consistency","call_id":"call_consistency",
                "name":"tool","arguments":"{}","status":"completed"})
        } else {
            assistant()
        };
        state
            .translate_value(
                json!({"type":"response.output_item.done","output_index":0,"item":output}),
            )
            .await
            .unwrap();
        let mut footer = caller_copy(output.clone());
        footer["id"] = output["id"].clone();
        let terminal = state
            .translate_value(json!({"type":"response.completed",
            "response":{"id":"resp_inconsistent","output":[footer]}}))
            .await
            .unwrap();
        let mut next = request(json!([input("continue")]));
        next["previous_response_id"] = terminal["response"]["id"].clone();
        assert!(matches!(
            prepare(&store, next).await,
            Err(StatefulPrepareError::StateUnavailable)
        ));
    }
}

#[tokio::test]
async fn filtered_subscription_controls_do_not_split_a_completed_history() {
    let (_temp, store) = store();
    let mut first = request(json!([input("first")]));
    first["temperature"] = 0.1.into();
    first["max_output_tokens"] = 256.into();
    first["metadata"] = json!({"request":"one"});
    first["unknown_client_option"] = "one".into();
    let prepared = prepare(&store, first.clone()).await.unwrap();
    let response = publish(&store, prepared, "resp_filtered", json!([assistant()])).await;
    first["input"] = json!([input("first"), response["output"][0], input("second")]);
    first["temperature"] = 0.9.into();
    first["max_output_tokens"] = 512.into();
    first["metadata"] = json!({"request":"two"});
    first["unknown_client_option"] = "two".into();
    assert!(
        plan(&store, &first, KEY).unwrap().baseline.is_some(),
        "discarded fields cannot alter the effective request configuration"
    );
    for field in ["instructions", "model", "service_tier"] {
        let mut changed = first.clone();
        changed[field] = "different".into();
        assert!(
            plan(&store, &changed, KEY).unwrap().baseline.is_some(),
            "current {field} split verified history"
        );
    }
}

#[tokio::test]
async fn omitted_tool_item_id_candidates_are_filtered_by_suffix_reference_dependencies() {
    let (_temp, store) = store();
    let first = prepare(&store, request(json!([input("first")])))
        .await
        .unwrap();
    let second = prepare(&store, request(json!([input("first")])))
        .await
        .unwrap();
    let second_identity = second.resolved_identity.as_ref().unwrap().clone();
    let output = |id| {
        json!([{"type":"function_call","id":id,"call_id":"shared-call",
        "name":"tool","arguments":"{}","status":"completed"}])
    };
    publish(&store, first, "resp_candidate_one", output("fc_one")).await;
    let response = publish(&store, second, "resp_candidate_two", output("fc_two")).await;
    let next = request(
        json!([input("first"), caller_copy(response["output"][0].clone()),
        {"type":"item_reference","id":response["output"][0]["id"]}]),
    );
    let selected = plan(&store, &next, KEY).unwrap();
    assert_eq!(
        selected.baseline.as_ref().unwrap().identity.session_id,
        second_identity.session_id
    );
}

#[test]
fn simplified_caller_history_does_not_expand_ws_automatic_reuse_eligibility() {
    use crate::request_profile::CallerKind;
    use crate::responses_websocket_state::{PublicCreateMode, ResponsesWebSocketState};
    for reduced in [false, true] {
        let mut state =
            ResponsesWebSocketState::new(CallerKind::Bare, UpstreamProfile::CodexSubscription1534);
        let user = json!({"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]});
        let mut first = request(json!([user]));
        first["type"] = "response.create".into();
        first["client_metadata"] = json!({"thread_id":"thread"});
        state.plan_public_create(&first);
        assert!(state.mark_public_create_attempted());
        state.observe_server_event(&json!({"type":"response.completed",
            "response":{"id":"resp_ws","output":[assistant()]}}));
        first["input"] = json!([user, if reduced { caller_copy(assistant()) } else { assistant() },
            {"type":"message","role":"user","content":[{"type":"input_text","text":"next"}]}]);
        let selected = state.plan_public_create(&first);
        assert!(matches!(selected.mode, PublicCreateMode::Full) == reduced);
        assert!(matches!(selected.mode, PublicCreateMode::Incremental) != reduced);
    }
}

#[tokio::test]
async fn direct_tool_items_require_a_nonblank_call_id_before_inference() {
    for kind in [
        "function_call",
        "custom_tool_call",
        "function_call_output",
        "custom_tool_call_output",
    ] {
        for call in [None, Some(Value::Null), Some(json!("")), Some(json!(" \t"))] {
            let (_temp, store) = store();
            let mut item = json!({"type":kind,"name":"tool","arguments":"{}","input":"data","output":"result"});
            if let Some(call) = call {
                item["call_id"] = call;
            }
            let body = request(json!([item]));
            assert!(
                matches!(
                    prepare(&store, body).await,
                    Err(StatefulPrepareError::InvalidRequest)
                ),
                "a direct tool item without its call reference was admitted"
            );
        }
    }
}

#[tokio::test]
async fn formed_lite_history_compares_defaults_for_its_selected_format() {
    let (_temp, store) = store();
    let mut body = json!({"model":"gpt-5.4","input":[
        {"type":"additional_tools","role":"developer","tools":[]},
        {"role":"developer","content":"native Lite base"}, input("first") ]});
    let first = prepare(&store, body.clone()).await.unwrap();
    let upstream: Value = serde_json::from_slice(&first.body).unwrap();
    assert_eq!(upstream["parallel_tool_calls"], false);
    assert_eq!(upstream["reasoning"]["context"], "all_turns");
    let response = publish(&store, first, "resp_forced_lite", json!([assistant()])).await;
    body["input"]
        .as_array_mut()
        .unwrap()
        .extend([response["output"][0].clone(), input("second")]);
    body["parallel_tool_calls"] = false.into();
    body["reasoning"] = json!({"context":"all_turns"});
    assert!(
        plan(&store, &body, KEY).unwrap().baseline.is_some(),
        "explicit Lite defaults must match the same defaults filled during the original send"
    );
}
