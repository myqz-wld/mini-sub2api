use super::*;
use crate::request_identity_projection::ResolvedRequestIdentity;

fn compact(items: Vec<Value>, v2: bool) -> Value {
    let mut body = request(Value::Array(items));
    body["client_metadata"] = json!({"x-codex-turn-metadata":json!({
        "request_kind":"compaction","compaction":{"implementation":if v2 {
            "responses_compaction_v2"
        } else {
            "responses"
        }}
    }).to_string()});
    if v2 {
        body["input"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"compaction_trigger"}));
    }
    body
}

fn tool_items() -> Vec<Value> {
    vec![
        json!({"type":"reasoning","id":"rs_recovery","summary":[],"encrypted_content":"synthetic-ciphertext"}),
        json!({"type":"function_call","id":"fc_recovery","call_id":"call_function_recovery","name":"probe","arguments":"{}"}),
        json!({"type":"custom_tool_call","id":"ctc_recovery","call_id":"call_custom_recovery","name":"custom_probe","input":"synthetic input"}),
    ]
}

fn complete_history(output: &[Value]) -> Vec<Value> {
    let mut items = vec![input("Synthetic recovery source")];
    items.extend_from_slice(output);
    items.push(json!({"type":"function_call_output","call_id":output[1]["call_id"],"output":"synthetic function result"}));
    items.push(json!({"type":"custom_tool_call_output","call_id":output[2]["call_id"],"output":"synthetic custom result"}));
    items
}

async fn seed_tools(store: &RequestStateStore) -> (ResolvedRequestIdentity, Vec<Value>) {
    let mut body = request(json!([input("Synthetic recovery source")]));
    body["include"] = json!(["reasoning.encrypted_content"]);
    let prepared = prepare(store, body).await.unwrap();
    let identity = prepared.resolved_identity.as_ref().unwrap().clone();
    let mut output = tool_items();
    for item in &mut output {
        item["internal_chat_message_metadata_passthrough"] = json!({"turn_id":identity.turn_id});
    }
    let response = publish(store, prepared, "resp_recovery_source", json!(output)).await;
    (
        identity,
        complete_history(response["output"].as_array().unwrap()),
    )
}

fn assert_preserved_tools(wire: &Value) {
    let items = wire["input"].as_array().unwrap();
    for expected in tool_items() {
        let actual = items
            .iter()
            .find(|item| item["type"] == expected["type"])
            .unwrap();
        assert_eq!(actual["id"], expected["id"]);
        if expected["type"] == "reasoning" {
            assert_eq!(actual["encrypted_content"], expected["encrypted_content"]);
        } else {
            assert_eq!(actual["call_id"], expected["call_id"]);
            let result = items
                .iter()
                .find(|item| {
                    item["type"]
                        .as_str()
                        .is_some_and(|kind| kind.ends_with("_output"))
                        && item["call_id"] == actual["call_id"]
                })
                .expect("paired tool result");
            assert!(result["output"].as_str().unwrap().starts_with("synthetic"));
        }
    }
}

async fn finish_compaction(
    store: &RequestStateStore,
    prepared: PreparedEmulatedRequest,
    v2: bool,
) -> Value {
    let state = ResponseStateContext::new(
        OWNER,
        NAMESPACE,
        KEY,
        store,
        prepared.resolved_identity.as_ref(),
        prepared.pending_compaction.as_ref(),
    )
    .with_operation(prepared.operation);
    let id = "resp_recovered_compaction";
    let output = if v2 {
        json!({"type":"compaction","id":"cmp_recovery","encrypted_content":"synthetic checkpoint"})
    } else {
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic summary"}]})
    };
    for event in [
        json!({"type":"response.created","response":{"id":id}}),
        json!({"type":"response.output_item.done","output_index":0,"item":output}),
    ] {
        state.translate_value(event).await.unwrap();
    }
    state
        .translate_value(
            json!({"type":"response.completed","response":{"id":id,"output":[output]}}),
        )
        .await
        .unwrap()["response"]
        .clone()
}

#[tokio::test]
async fn compaction_import_recovers_expired_and_restarted_tool_history() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for model in ["gpt-5.4", "gpt-6-astra"] {
            for restart in [false, true] {
                for v2 in [false, true] {
                    let (temp, original) = store();
                    let (source, history) = seed_tools(&original).await;
                    let store = if restart {
                        RequestStateStore::new(temp.path().to_path_buf())
                    } else {
                        expire(&original);
                        original
                    };
                    let mut body = compact(history, v2);
                    body["model"] = model.into();
                    let prepared = prepare_transport(&store, body, transport, KEY)
                        .await
                        .expect(
                            "complete compaction history must recover without a retained baseline",
                        );
                    let target = prepared.resolved_identity.as_ref().unwrap().clone();
                    assert_ne!(target.thread_id, source.thread_id);
                    assert_eq!(target.request_kind, "compaction");
                    assert_eq!(target.window_number, 0);
                    assert_eq!(
                        prepared.pending_compaction.as_ref().unwrap().target_window,
                        1
                    );
                    let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
                    assert_preserved_tools(&wire);
                    let imported = wire["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|item| item["type"] == "reasoning")
                        .unwrap()["internal_chat_message_metadata_passthrough"]["turn_id"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    assert_ne!(Some(imported.as_str()), source.turn_id.as_deref());
                    assert_eq!(
                        wire["input"].as_array().unwrap().last().unwrap()["type"]
                            == "compaction_trigger",
                        v2
                    );
                    let target_thread = target.thread_id.clone();
                    store
                        .edit(NAMESPACE, OWNER, KEY, move |editor| {
                            assert_eq!(
                                editor
                                    .turn_by_id(source.turn_id.as_deref().unwrap())
                                    .unwrap()
                                    .1
                                    .thread_id,
                                source.thread_id
                            );
                            assert_eq!(
                                editor.turn_by_id(&imported).unwrap().1.thread_id,
                                target_thread
                            );
                            Ok(())
                        })
                        .await
                        .unwrap();

                    let completed = finish_compaction(&store, prepared, v2).await;
                    let mut next = request(json!([input("Continue after recovery")]));
                    next["model"] = model.into();
                    if v2 {
                        next["previous_response_id"] = completed["id"].clone();
                    } else {
                        // Native local compaction installs its client-owned summary wrapper.
                        next["client_metadata"] = json!({"session_id":target.session_id});
                        next["input"] = json!([
                            input("Synthetic recovery source"),
                            input("Synthetic client summary wrapper"),
                            input("Continue after recovery")
                        ]);
                    }
                    let next = prepare_transport(&store, next, transport, KEY)
                        .await
                        .unwrap();
                    assert_eq!(
                        next.resolved_identity.as_ref().unwrap().thread_id,
                        target.thread_id
                    );
                    assert_eq!(next.resolved_identity.as_ref().unwrap().window_number, 1);
                    let wire: Value = serde_json::from_slice(&next.body).unwrap();
                    assert!(wire.get("previous_response_id").is_none());
                    assert!(wire["input"].as_array().unwrap().iter().all(|item| {
                        !matches!(
                            item["type"].as_str(),
                            Some("function_call" | "custom_tool_call" | "compaction_trigger")
                        )
                    }));
                }
            }
        }
    }
}

#[tokio::test]
async fn compaction_import_pseudonymizes_unverified_first_replay_ids() {
    for model in ["gpt-5.4", "gpt-6-astra"] {
        for v2 in [false, true] {
            let (_temp, store) = store();
            let mut body = compact(complete_history(&tool_items()), v2);
            body["model"] = model.into();
            let prepared = prepare(&store, body).await.unwrap();
            let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
            let items = wire["input"].as_array().unwrap();
            for expected in tool_items() {
                let actual = items
                    .iter()
                    .find(|item| item["type"] == expected["type"])
                    .unwrap();
                assert_ne!(actual["id"], expected["id"]);
                if expected.get("call_id").is_some() {
                    assert_ne!(actual["call_id"], expected["call_id"]);
                    assert!(items.iter().any(|result| {
                        result["type"]
                            .as_str()
                            .is_some_and(|t| t.ends_with("_output"))
                            && result["call_id"] == actual["call_id"]
                    }));
                } else {
                    assert_eq!(actual["encrypted_content"], expected["encrypted_content"]);
                }
            }
        }
    }
}

#[tokio::test]
async fn compaction_import_rejects_incomplete_or_unowned_context_without_mutation() {
    for v2 in [false, true] {
        for case in [
            "retained",
            "previous",
            "explicit-session",
            "explicit-turn",
            "thread",
            "external-conversation",
            "item-reference",
            "missing-call",
            "open-call",
            "duplicate-result",
            "missing-cipher",
            "empty-cipher",
            "conflicting-id",
            "repeated-trigger",
            "middle-trigger",
            "trigger-only",
            "ordinary-trigger",
        ] {
            let (_temp, store) = store();
            let (turn, _, previous) = seed(&store).await;
            if case != "retained" {
                expire(&store);
            }
            let mut items = complete_history(&tool_items());
            items.insert(1, historical(&turn));
            let mut body = compact(items, v2);
            match case {
                "previous" => body["previous_response_id"] = previous.into(),
                "explicit-session" => {
                    body["client_metadata"]["session_id"] = "unrelated-session".into()
                }
                "explicit-turn" => body["client_metadata"]["turn_id"] = "unrelated-turn".into(),
                "thread" => body["client_metadata"]["thread_id"] = "unrelated-thread".into(),
                "external-conversation" => {
                    body["conversation"] = json!({"id":"remote-conversation"})
                }
                "item-reference" => body["input"]
                    .as_array_mut()
                    .unwrap()
                    .insert(2, json!({"type":"item_reference","id":"msg_history_copy"})),
                "missing-call" => {
                    body["input"].as_array_mut().unwrap().remove(3);
                }
                "open-call" => {
                    body["input"].as_array_mut().unwrap().remove(5);
                }
                "duplicate-result" => {
                    let result = body["input"][5].clone();
                    body["input"].as_array_mut().unwrap().insert(5, result);
                }
                "missing-cipher" => {
                    body["input"][2]
                        .as_object_mut()
                        .unwrap()
                        .remove("encrypted_content");
                }
                "empty-cipher" => body["input"][2]["encrypted_content"] = "".into(),
                "conflicting-id" => {
                    let mut item = historical(&turn);
                    item["content"][0]["text"] = "conflicting".into();
                    body["input"].as_array_mut().unwrap().insert(2, item);
                }
                "repeated-trigger" | "middle-trigger" | "ordinary-trigger" => {
                    let items = body["input"].as_array_mut().unwrap();
                    if !v2 {
                        items.push(json!({"type":"compaction_trigger"}));
                    }
                    if case == "repeated-trigger" {
                        items.push(json!({"type":"compaction_trigger"}));
                    }
                    if case == "middle-trigger" {
                        items.push(input("after trigger"));
                    }
                    if case == "ordinary-trigger" {
                        body["client_metadata"] = json!({});
                    }
                }
                "trigger-only" => {
                    body["input"] = json!([historical(&turn), {"type":"compaction_trigger"}])
                }
                _ => {}
            }
            let before = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
            assert!(
                prepare(&store, body).await.is_err(),
                "accepted {case}, v2={v2}"
            );
            assert_eq!(
                std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
                before,
                "mutated {case}, v2={v2}"
            );
            let inner = store.contexts.inner.lock().unwrap();
            assert!(inner.reservations.is_empty());
            assert!(inner.operations.is_empty());
        }
    }
}

#[tokio::test]
async fn compaction_import_never_adopts_an_active_source() {
    for v2 in [false, true] {
        let (_temp, store) = store();
        let active = prepare(&store, request(json!([input("Running source")])))
            .await
            .unwrap();
        let turn = active
            .resolved_identity
            .as_ref()
            .unwrap()
            .turn_id
            .as_deref()
            .unwrap();
        let body = compact(vec![input("Running source"), historical(turn)], v2);
        let before = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
        assert!(prepare(&store, body).await.is_err());
        assert_eq!(
            std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
            before
        );
        let inner = store.contexts.inner.lock().unwrap();
        assert!(inner.reservations.is_empty());
        assert_eq!(inner.operations.len(), 1);
    }
}
