use super::*;

#[tokio::test]
async fn current_settings_do_not_split_verified_anonymous_history() {
    for (field, value) in [
        ("instructions", json!("  current base {{caller}}  ")),
        (
            "tools",
            json!([{"type":"function","name":"current_tool","parameters":{"type":"object"}}]),
        ),
        ("model", json!("gpt-5.5")),
        ("reasoning", json!({"effort":"high","summary":"detailed"})),
        ("tool_choice", json!("none")),
        ("parallel_tool_calls", json!(false)),
        (
            "include",
            json!([
                "reasoning.encrypted_content",
                "message.output_text.logprobs"
            ]),
        ),
        ("service_tier", json!("priority")),
        ("prompt_cache_key", json!("current-cache")),
        ("text", json!({"verbosity":"low"})),
    ] {
        let (_temp, store) = store();
        let first = prepare(&store, request(json!([input("first")])))
            .await
            .unwrap();
        let identity = first.resolved_identity.as_ref().unwrap().clone();
        let response = publish(&store, first, "resp_configuration", json!([assistant()])).await;
        let mut next = request(json!([
            input("first"),
            response["output"][0],
            input("next")
        ]));
        next[field] = value.clone();
        assert!(
            plan(&store, &next, KEY).unwrap().baseline.is_some(),
            "configuration split history: {field}"
        );
        let prepared = prepare(&store, next).await.unwrap();
        let current = prepared.resolved_identity.as_ref().unwrap();
        assert_eq!(current.session_id, identity.session_id);
        assert_eq!(current.thread_id, identity.thread_id);
        assert_ne!(current.turn_id, identity.turn_id);
        let emitted: Value = serde_json::from_slice(&prepared.body).unwrap();
        if field == "tools" {
            assert_eq!(emitted[field].as_array().unwrap().len(), 1);
            assert!(emitted[field][0]["name"] == "current_tool");
            assert!(emitted[field][0]["parameters"]["type"] == "object");
        } else if field == "tool_choice" {
            assert_eq!(emitted[field], "auto");
        } else if field == "include" {
            assert_eq!(emitted[field], json!(["reasoning.encrypted_content"]));
        } else if field == "prompt_cache_key" {
            assert!(emitted[field] == current.session_id);
        } else {
            assert!(
                emitted[field] == value,
                "current setting was not sent: {field}"
            );
        }
        assert!(emitted.get("previous_response_id").is_none());
    }
}

#[tokio::test]
async fn model_and_setup_changes_keep_current_format_without_inheriting_old_bases() {
    for (old_model, model) in [("gpt-5.4", "gpt-5.6-sol"), ("gpt-5.6-sol", "gpt-5.4")] {
        for base in [
            None,
            Some(Value::Null),
            Some(json!(" \n")),
            Some(json!("current base")),
        ] {
            let (_temp, store) = store();
            let mut first = request(json!([input("first")]));
            first["model"] = old_model.into();
            first["tools"] =
                json!([{"type":"function","name":"old_tool","parameters":{"type":"object"}}]);
            let first = prepare(&store, first).await.unwrap();
            let identity = first.resolved_identity.as_ref().unwrap().clone();
            let response = publish(&store, first, "resp_old_format", json!([assistant()])).await;
            let mut next =
                json!({"model":model,"input":[input("first"),response["output"][0],input("next")]});
            if let Some(base) = &base {
                next["instructions"] = base.clone();
            }
            let selected = plan(&store, &next, KEY).unwrap();
            assert!(selected.baseline.is_some());
            assert_eq!(
                selected.caller_format,
                crate::subscription_request::Format::Responses
            );
            drop(selected);
            let prepared = prepare(&store, next).await.unwrap();
            assert_eq!(
                prepared.resolved_identity.as_ref().unwrap().session_id,
                identity.session_id
            );
            let emitted: Value = serde_json::from_slice(&prepared.body).unwrap();
            let current_base = base
                .as_ref()
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let has_base = current_base.is_some();
            let items = emitted["input"].as_array().unwrap();
            if model == "gpt-5.6-sol" {
                assert!(emitted.get("instructions").is_none());
                assert!(items[0]["tools"] == json!([]));
                assert_eq!(items.len(), 4 + usize::from(has_base));
                if has_base {
                    assert_eq!(items[1]["content"][0]["text"].as_str(), current_base);
                }
            } else {
                assert_eq!(items.len(), 3);
                assert_eq!(
                    emitted.get("instructions").and_then(Value::as_str),
                    current_base
                );
                assert_eq!(emitted["tools"], json!([]));
            }
            assert!(
                !items
                    .iter()
                    .any(|item| item["content"][0]["text"] == "base")
            );
        }
    }
}

#[tokio::test]
async fn longest_history_wins_even_when_only_the_shorter_record_has_current_settings() {
    let (_temp, store) = store();
    let first = prepare(&store, request(json!([input("first")])))
        .await
        .unwrap();
    let first = publish(&store, first, "resp_config_short", json!([assistant()])).await;
    let mut second = request(json!([input("first"), first["output"][0], input("second")]));
    second["instructions"] = "different base".into();
    let prepared = prepare(&store, second.clone()).await.unwrap();
    let response = publish(&store, prepared, "resp_config_long", json!([assistant()])).await;
    second["input"]
        .as_array_mut()
        .unwrap()
        .extend([response["output"][0].clone(), input("third")]);
    second["instructions"] = "base".into();
    let selected = plan(&store, &second, KEY).unwrap();
    assert_eq!(selected.baseline.unwrap().history.unwrap().len, 4);
}

#[tokio::test]
async fn equivalent_histories_with_different_settings_keep_independent_execution_owners() {
    let (_temp, store) = store();
    let mut body = request(json!([input("first")]));
    let first = prepare(&store, body.clone()).await.unwrap();
    body["instructions"] = "another base".into();
    let second = prepare(&store, body).await.unwrap();
    let owners = [
        first.resolved_identity.as_ref().unwrap().session_id.clone(),
        second
            .resolved_identity
            .as_ref()
            .unwrap()
            .session_id
            .clone(),
    ];
    let call = |id| json!([{"type":"function_call","id":id,"call_id":"shared-call","name":"tool","arguments":"{}"}]);
    let one = publish(&store, first, "resp_config_one", call("fc_one")).await;
    publish(&store, second, "resp_config_two", call("fc_two")).await;
    let item = caller_copy(one["output"][0].clone());
    let body =
        json!({"model":"gpt-5.4","instructions":"third base","input":[input("first"), item]});
    let matched = plan(&store, &body, KEY).unwrap();
    let source = matched
        .baseline
        .as_ref()
        .expect("must associate existing history");
    assert_ne!(matched.branch.as_ref().unwrap(), &source.branch);
    drop(matched);
    let first = prepare(&store, body.clone()).await.unwrap();
    let second = prepare(&store, body).await.unwrap();
    for prepared in [&first, &second] {
        assert!(owners.contains(&prepared.resolved_identity.as_ref().unwrap().session_id));
    }
    let a = first.operation.as_ref().unwrap();
    let b = second.operation.as_ref().unwrap();
    store.contexts.learn_turn(a, "first-owner-token").unwrap();
    assert!(store.contexts.turn_token(b).is_none());
    let inner = store.contexts.inner.lock().unwrap();
    assert_ne!(
        inner.operations[&a.0.id].lane,
        inner.operations[&b.0.id].lane
    );
    assert!(
        inner.operations[&a.0.id]
            .record
            .dependencies
            .awaiting_tools()
    );
    assert!(
        inner.operations[&b.0.id]
            .record
            .dependencies
            .awaiting_tools()
    );
}

#[tokio::test]
async fn history_dependency_conflicts_still_reject_across_different_configurations() {
    let (_temp, store) = store();
    let mut body = request(json!([input("first")]));
    let first = prepare(&store, body.clone()).await.unwrap();
    body["instructions"] = "second base".into();
    let second = prepare(&store, body).await.unwrap();
    let call = |id| json!([{"type":"function_call","id":id,"call_id":"shared","name":"tool","arguments":"{}"}]);
    let one = publish(&store, first, "resp_conflict_one", call("fc_one")).await;
    let two = publish(&store, second, "resp_conflict_two", call("fc_two")).await;
    // Challenge the defensive consistency check with contradictory saved consumption facts.
    // This is a synthetic cache-state fault, not a claim about normal upstream output.
    {
        let mut inner = store.contexts.inner.lock().unwrap();
        let scope = inner
            .scopes
            .get_mut(&ContextStore::scope_key(NAMESPACE, KEY))
            .unwrap();
        let record = scope.records.get_mut(two["id"].as_str().unwrap()).unwrap();
        for consumed in record.dependencies.calls.values_mut() {
            *consumed = true;
        }
    }
    let mut next = request(json!([
        input("first"),
        caller_copy(one["output"][0].clone()),
        input("next")
    ]));
    next["instructions"] = "third base".into();
    assert!(matches!(
        plan(&store, &next, KEY),
        Err(StatefulPrepareError::StateUnavailable)
    ));
}

#[tokio::test]
async fn changed_configuration_keeps_tool_turn_and_consumption_validation() {
    let (_temp, store) = store();
    let first = prepare(&store, request(json!([input("first")])))
        .await
        .unwrap();
    let identity = first.resolved_identity.as_ref().unwrap().clone();
    let response = publish(&store, first, "resp_config_tool", json!([
        {"type":"function_call","id":"fc_config","call_id":"call_config","name":"tool","arguments":"{}"}
    ])).await;
    let result = json!({"type":"function_call_output","call_id":response["output"][0]["call_id"],"output":"result"});
    let mut next = json!({"model":"gpt-5.6-sol","instructions":"changed","tools":[],
        "input":[input("first"),response["output"][0],result]});
    let prepared = prepare(&store, next.clone()).await.unwrap();
    let current = prepared.resolved_identity.as_ref().unwrap();
    assert_eq!(current.session_id, identity.session_id);
    assert_eq!(current.turn_id, identity.turn_id);
    let response = publish(&store, prepared, "resp_config_consumed", json!([])).await;
    next["previous_response_id"] = response["id"].clone();
    next["input"] = json!([result]);
    assert!(matches!(
        prepare(&store, next).await,
        Err(StatefulPrepareError::InvalidRequest)
    ));
}

#[tokio::test]
async fn configuration_independent_lookup_preserves_id_content_and_anonymous_scope_boundaries() {
    for explicit in [false, true] {
        let (_temp, store) = store();
        let mut first =
            request(json!([{"role":"developer","content":"history rule"},input("first")]));
        if explicit {
            first["client_metadata"] = json!({"session_id":"explicit-session"});
        }
        let prepared = prepare(&store, first.clone()).await.unwrap();
        let response = publish(
            &store,
            prepared,
            "resp_config_boundary",
            json!([assistant()]),
        )
        .await;
        let mut next = first;
        next.as_object_mut().unwrap().remove("client_metadata");
        next["instructions"] = "different".into();
        next["input"]
            .as_array_mut()
            .unwrap()
            .extend([response["output"][0].clone(), input("next")]);
        assert_eq!(
            plan(&store, &next, KEY).unwrap().baseline.is_some(),
            !explicit
        );
        assert!(plan(&store, &next, "other-key").unwrap().baseline.is_none());
        let mut other_session = next.clone();
        other_session["client_metadata"] = json!({"session_id":"unrelated-session"});
        assert!(
            plan(&store, &other_session, KEY)
                .unwrap()
                .baseline
                .is_none()
        );
        next["input"][2]["id"] = "conflicting-id".into();
        assert!(plan(&store, &next, KEY).unwrap().baseline.is_none());
        next["input"][2] = response["output"][0].clone();
        next["input"][0]["content"] = "edited history rule".into();
        assert!(plan(&store, &next, KEY).unwrap().baseline.is_none());
    }
}

#[tokio::test]
async fn formed_lite_current_settings_are_separate_from_its_actual_input_prefix() {
    let (_temp, store) = store();
    let mut first = json!({"model":"gpt-5.4","input":[
        {"type":"additional_tools","role":"developer","tools":[]},
        {"role":"developer","content":"formed base"}, input("first")]});
    let prepared = prepare(&store, first.clone()).await.unwrap();
    let identity = prepared.resolved_identity.as_ref().unwrap().clone();
    let response = publish(&store, prepared, "resp_config_formed", json!([assistant()])).await;
    first["model"] = "gpt-5.6-sol".into();
    first["reasoning"] = json!({"effort":"high","context":"all_turns"});
    first["input"]
        .as_array_mut()
        .unwrap()
        .extend([response["output"][0].clone(), input("next")]);
    let prepared = prepare(&store, first.clone()).await.unwrap();
    assert_eq!(
        prepared.resolved_identity.as_ref().unwrap().session_id,
        identity.session_id
    );
    let body: Value = serde_json::from_slice(&prepared.body).unwrap();
    assert!(body["reasoning"]["effort"] == "high");
    assert!(body["input"][1]["content"][0]["text"] == "formed base");
    drop(prepared);
    first["input"][1]["content"] = "edited input base".into();
    assert!(plan(&store, &first, KEY).unwrap().baseline.is_none());
}

#[tokio::test]
async fn disabled_configuration_updates_remain_in_stored_history_only() {
    let (_temp, store) = store();
    let update = json!({"type":"configuration_update","reasoning":{"effort":"high"}});
    let first = prepare(&store, request(json!([update, input("first")])))
        .await
        .unwrap();
    let session = first.resolved_identity.as_ref().unwrap().session_id.clone();
    let wire: Value = serde_json::from_slice(&first.body).unwrap();
    assert!(
        wire["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["type"] != "configuration_update")
    );
    let response = publish(&store, first, "resp_update_history", json!([assistant()])).await;
    let next = request(json!([
        update,
        input("first"),
        response["output"][0],
        input("next")
    ]));
    let plan = plan(&store, &next, KEY).unwrap();
    assert!(
        plan.baseline
            .as_ref()
            .unwrap()
            .history
            .as_ref()
            .unwrap()
            .values()
            .contains(&update)
    );
    drop(plan);
    let prepared = prepare(&store, next).await.unwrap();
    assert_eq!(
        prepared.resolved_identity.as_ref().unwrap().session_id,
        session
    );
    let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
    assert!(
        wire["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["type"] != "configuration_update")
    );
}
