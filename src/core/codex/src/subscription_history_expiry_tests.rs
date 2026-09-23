//! Complete caller replay imports expired turn metadata without reassigning source owners.
use super::*;
use std::time::{Duration, Instant};

#[tokio::test]
async fn full_history_expiry_imports_old_turns_without_reassigning_source_ownership() {
    for restart in [false, true] {
        for explicit_session in [false, true] {
            for keep_turn in [false, true] {
                for tool in [false, true] {
                    let (temp, initial) = store();
                    let mut first = request(json!([input("Synthetic first turn")]));
                    if explicit_session {
                        first["client_metadata"] = json!({"session_id":"stable-session"});
                    }
                    let prepared = prepare(&initial, first.clone()).await.unwrap();
                    let original = prepared.resolved_identity.as_ref().unwrap().clone();
                    let output = if tool {
                        json!({"type":"function_call","id":"fc_expiry","call_id":"call_expiry",
                            "name":"inspect","arguments":"{}"})
                    } else {
                        json!({"type":"message","id":"msg_expiry","role":"assistant",
                            "status":"completed","content":[{"type":"output_text","text":"Synthetic answer",
                                "annotations":[],"logprobs":[]}]})
                    };
                    let mut output = output;
                    output["internal_chat_message_metadata_passthrough"] =
                        json!({"turn_id":original.turn_id});
                    let response =
                        publish(&initial, prepared, "resp_expiry", json!([output])).await;
                    let mut returned = response["output"][0].clone();
                    assert!(returned["id"].is_string());
                    if keep_turn {
                        assert!(
                            returned["internal_chat_message_metadata_passthrough"]["turn_id"]
                                .is_string()
                        );
                    } else {
                        returned
                            .as_object_mut()
                            .unwrap()
                            .remove("internal_chat_message_metadata_passthrough");
                    }
                    let mut history = vec![input("Synthetic first turn"), returned.clone()];
                    if tool {
                        history.push(
                            json!({"type":"function_call_output","call_id":returned["call_id"],
                            "output":"Synthetic tool result"}),
                        );
                    }
                    history.push(input("Synthetic next-day continuation"));
                    first["input"] = Value::Array(history);
                    let store = if restart {
                        RequestStateStore::new(temp.path().to_path_buf())
                    } else {
                        initial
                            .contexts
                            .inner
                            .lock()
                            .unwrap()
                            .expire(Instant::now() + Duration::from_secs(4 * 60 * 60));
                        initial
                    };
                    let replay = prepare(&store, first.clone())
                        .await
                        .expect("complete expired history can be replayed");
                    let target = replay.resolved_identity.as_ref().unwrap().clone();
                    assert_eq!(target.session_id == original.session_id, explicit_session);
                    let wire: Value = serde_json::from_slice(&replay.body).unwrap();
                    assert!(wire.get("previous_response_id").is_none());
                    assert_eq!(
                        wire["input"][1]["id"],
                        if tool { "fc_expiry" } else { "msg_expiry" }
                    );
                    if tool {
                        assert_eq!(wire["input"][1]["call_id"], "call_expiry");
                        assert_eq!(wire["input"][2]["call_id"], "call_expiry");
                    }
                    let replayed_turn =
                        wire["input"][1]["internal_chat_message_metadata_passthrough"]["turn_id"]
                            .as_str()
                            .unwrap()
                            .to_string();
                    if keep_turn && !explicit_session {
                        assert_ne!(Some(replayed_turn.as_str()), original.turn_id.as_deref());
                        let old = original.clone();
                        let imported = replayed_turn.clone();
                        let target = target.clone();
                        store
                            .edit(NAMESPACE, OWNER, KEY, move |editor| {
                                let (_, source) =
                                    editor.turn_by_id(old.turn_id.as_deref().unwrap()).unwrap();
                                assert_eq!(source.thread_id, old.thread_id);
                                let (_, copy) = editor.turn_by_id(&imported).unwrap();
                                assert_eq!(copy.thread_id, target.thread_id);
                                assert_ne!(copy.id, source.id);
                                Ok(())
                            })
                            .await
                            .unwrap();
                    }
                    let completed = publish(&store, replay, "resp_imported", json!([])).await;
                    first["input"]
                        .as_array_mut()
                        .unwrap()
                        .push(input("Continue imported history"));
                    let next = prepare(&store, first.clone()).await.unwrap();
                    assert_eq!(
                        next.resolved_identity.as_ref().unwrap().session_id,
                        target.session_id
                    );
                    let next_wire: Value = serde_json::from_slice(&next.body).unwrap();
                    assert_eq!(
                        next_wire["input"][1]["internal_chat_message_metadata_passthrough"]["turn_id"],
                        replayed_turn
                    );
                    drop(next);
                    let delta = json!({"model":"gpt-5.4","previous_response_id":completed["id"],"input":[input("Referenced continuation")]});
                    assert!(prepare(&store, delta).await.is_ok());
                }
            }
        }
    }
}
