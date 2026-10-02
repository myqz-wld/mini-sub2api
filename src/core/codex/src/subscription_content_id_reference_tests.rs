use super::*;
use crate::request_state_types::{PersistedRequestState, WireIdDomain};

#[tokio::test]
async fn provider_aliases_keep_original_upstream_ids_on_history_replay() {
    let (_temp, store) = store();
    let first = prepare(
        &store,
        request(json!([input("Synthetic provider history")])),
    )
    .await
    .unwrap();
    let response = publish(&store, first, "resp_content_provider", json!([
        {"type":"message","id":"msg_provider_original","role":"assistant","content":[{"type":"output_text","text":"synthetic answer"}]},
        {"type":"custom_tool_call","id":"ctc_provider_original","call_id":"call_provider_original","name":"probe","input":"synthetic tool input"}
    ])).await;
    let body = request(json!([
        input("Synthetic provider history"), response["output"][0], response["output"][1],
        {"type":"custom_tool_call_output","call_id":response["output"][1]["call_id"],"output":"synthetic result"}
    ]));
    let replay = prepare(&store, body).await.unwrap();
    let value = wire(&replay);
    assert_eq!(value["input"][1]["id"], "msg_provider_original");
    assert_eq!(value["input"][2]["id"], "ctc_provider_original");
    assert_eq!(value["input"][2]["call_id"], "call_provider_original");
    assert_eq!(value["input"][3]["call_id"], "call_provider_original");
}

#[tokio::test]
async fn websocket_delta_uses_local_call_binding_after_body_expiry() {
    for custom in [false, true] {
        let (_temp, store) = store();
        let socket = store.contexts.open_socket().unwrap();
        let mut body = local_history("local-delta", "synthetic invocation");
        body["input"].as_array_mut().unwrap().truncate(3);
        if custom {
            body["input"][2] = json!({"type":"custom_tool_call","id":"ctc_pi_2","call_id":"call_pi_1","name":"probe","input":"synthetic invocation"});
        }
        let first = repair_tests::websocket_request(&store, body, &socket.id, None)
            .await
            .unwrap();
        let identity = first.resolved_identity.as_ref().unwrap().clone();
        let first_wire = wire(&first);
        let response = publish(&store, first, "resp_local_delta", json!([])).await;
        {
            let mut inner = store.contexts.inner.lock().unwrap();
            let scope = inner
                .scopes
                .get_mut(&ContextStore::scope_key(NAMESPACE, KEY))
                .unwrap();
            scope
                .records
                .get_mut(response["id"].as_str().unwrap())
                .unwrap()
                .history = None;
            scope.rebuild_index();
        }
        let mut next = request(json!([{
            "type":if custom {"custom_tool_call_output"} else {"function_call_output"},
            "call_id":"call_pi_1","output":"synthetic result"
        }]));
        next["previous_response_id"] = response["id"].clone();
        let next = repair_tests::websocket_request(&store, next, &socket.id, Some(&identity))
            .await
            .unwrap();
        let next_wire = wire(&next);
        assert_eq!(next_wire["input"].as_array().unwrap().len(), 1);
        assert_eq!(
            next_wire["input"][0]["call_id"],
            first_wire["input"][2]["call_id"]
        );
    }
}

#[tokio::test]
async fn repeated_local_call_ids_pair_by_occurrence_and_all_aliases_stay_protected() {
    let (_temp, store) = store();
    let mut body = local_history("repeated-session", "synthetic first invocation");
    let second = local_history("repeated-session", "synthetic second invocation");
    let items = body["input"].as_array_mut().unwrap();
    items.pop();
    items.extend([second["input"][2].clone(), second["input"][3].clone()]);
    let first = prepare(&store, body.clone()).await.unwrap();
    let value = wire(&first);
    assert_ne!(value["input"][2]["call_id"], value["input"][4]["call_id"]);
    assert_ne!(value["input"][3]["id"], value["input"][5]["id"]);
    assert_eq!(value["input"][2]["call_id"], value["input"][3]["call_id"]);
    assert_eq!(value["input"][4]["call_id"], value["input"][5]["call_id"]);
    publish(&store, first, "resp_repeat_occurrences", json!([])).await;
    store
        .edit_at(
            NAMESPACE,
            OWNER,
            KEY,
            chrono::Utc::now().timestamp_millis() + 40 * 86_400_000,
            |_| Ok(()),
        )
        .await
        .unwrap();
    let ledger: PersistedRequestState =
        serde_json::from_slice(&std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap())
            .unwrap();
    let scope = ledger.scopes.values().next().unwrap();
    for index in [2, 4] {
        let upstream = value["input"][index]["call_id"].as_str().unwrap();
        assert!(
            scope
                .generated_items
                .values()
                .any(|item| item.id == upstream)
        );
        assert!(
            scope
                .wire_ids
                .values()
                .any(|pair| pair.domain == WireIdDomain::Call && pair.upstream_id == upstream)
        );
    }
    let replay = prepare(&store, body).await.unwrap();
    let replay = wire(&replay);
    for index in [2, 3, 4, 5] {
        assert_eq!(value["input"][index]["id"], replay["input"][index]["id"]);
        assert_eq!(
            value["input"][index]["call_id"],
            replay["input"][index]["call_id"]
        );
    }
}
