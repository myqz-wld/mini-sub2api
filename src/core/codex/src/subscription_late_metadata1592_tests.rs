use super::repair_tests::websocket_request;
use super::*;
use crate::request_profile::CallerKind;
use crate::responses_websocket_state::{PublicCreateMode, ResponsesWebSocketState};
use crate::tool_observation_budget::META;

const EXEC_CALL: &str = "call_synthetic_exec";
const RUNTIME_CELL: &str = "synthetic_runtime_cell";

fn late_store() -> (tempfile::TempDir, RequestStateStore) {
    let (temp, mut store) = store();
    let limits = std::sync::Arc::make_mut(&mut store.contexts.limits);
    limits.global_bytes = 8 * 1024 * 1024;
    limits.key_bytes = 4 * 1024 * 1024;
    limits.session_bytes = 2 * 1024 * 1024;
    (temp, store)
}

fn full_request(history: &[Value]) -> Value {
    let mut body = request(json!(history));
    body["client_metadata"] =
        json!({"session_id":"synthetic-late-session","turn_id":"synthetic-late-turn"});
    body
}

async fn prepare_full(
    store: &RequestStateStore,
    history: &[Value],
    socket: Option<&str>,
) -> PreparedEmulatedRequest {
    let mut body = full_request(history);
    match socket {
        Some(socket) => {
            body["type"] = "response.create".into();
            websocket_request(store, body, socket, None).await.unwrap()
        }
        None => prepare(store, body).await.unwrap(),
    }
}

fn truncation() -> Value {
    json!({"_codex_executed_tool_call_truncated":{"original_bytes":9000,"max_bytes":8192}})
}

fn exec_output(call: &Value) -> Value {
    json!({"type":"custom_tool_call_output","id":"ctco_synthetic_exec",
        "call_id":call,"output":"Synthetic yielded execution",
        META:{"cell_id":call,"executed_tool_calls":[
            {"name":"tools.synthetic_lookup","arguments":truncation()}]}})
}

fn wait_call(index: usize) -> Value {
    json!({"type":"function_call","id":format!("fc_synthetic_wait_{index}"),
        "call_id":format!("call_synthetic_wait_{index}"),"name":"wait",
        "arguments":json!({"cell_id":RUNTIME_CELL}).to_string()})
}

fn wait_output(call: &Value, origin: &Value) -> Value {
    json!({"type":"function_call_output","call_id":call,
        "output":RUNTIME_CELL,META:{"cell_id":origin,
        "executed_tool_calls":[],"tool_calls_complete":true}})
}

async fn start_exec(store: &RequestStateStore, socket: Option<&str>) -> Vec<Value> {
    let mut history = vec![input("Synthetic delayed nested result")];
    let first = prepare_full(store, &history, socket).await;
    let completed = publish(
        store,
        first,
        "resp_synthetic_exec",
        json!([
            {"type":"custom_tool_call","id":"ctc_synthetic_exec","call_id":EXEC_CALL,
             "name":"exec","input":"await tools.synthetic_lookup({})"}
        ]),
    )
    .await;
    let call = completed["output"][0].clone();
    assert_ne!(call["call_id"], EXEC_CALL);
    history.push(call.clone());
    history.push(exec_output(&call["call_id"]));
    history
}

fn wire(prepared: &PreparedEmulatedRequest) -> Value {
    serde_json::from_slice(&prepared.body).unwrap()
}

fn observed_exec(value: &Value) -> &Value {
    value["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "custom_tool_call_output")
        .unwrap()
}

fn assert_observations(value: &Value, result: Option<&Value>, waits: usize) {
    let output = observed_exec(value);
    assert_eq!(output["call_id"], EXEC_CALL);
    assert_eq!(output[META]["cell_id"], EXEC_CALL);
    assert_eq!(output["output"], "Synthetic yielded execution");
    let calls = output[META]["executed_tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["name"], "tools.synthetic_lookup");
    assert_eq!(calls[0]["arguments"], truncation());
    assert_eq!(calls[0].get("tool_result_metadata"), result);
    assert!(output[META].get("tool_calls_complete").is_none());
    let wait_outputs: Vec<_> = value["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .collect();
    assert_eq!(wait_outputs.len(), waits);
    for wait in wait_outputs {
        assert_ne!(wait["call_id"], EXEC_CALL);
        assert_eq!(wait[META]["cell_id"], EXEC_CALL);
        assert_eq!(wait["output"], RUNTIME_CELL);
        assert_eq!(wait[META]["executed_tool_calls"], json!([]));
        assert!(wait[META].get("tool_calls_complete").is_none());
    }
}

fn complete_reuse(state: &mut ResponsesWebSocketState, id: &str, output: &Value) {
    assert!(state.mark_public_create_attempted());
    state.observe_server_event(&json!({"type":"response.created","response":{"id":id}}));
    for (index, item) in output.as_array().unwrap().iter().enumerate() {
        state.observe_server_event(
            &json!({"type":"response.output_item.done","output_index":index,"item":item}),
        );
    }
    state.observe_server_event(
        &json!({"type":"response.completed","response":{"id":id,"output":output}}),
    );
}

#[tokio::test]
async fn late_truncated_results_survive_waits_and_control_ws_reuse() {
    for websocket in [false, true] {
        for waits in [1, 3] {
            let (_temp, store) = late_store();
            let socket = store.contexts.open_socket().unwrap();
            let socket_id = websocket.then_some(socket.id.as_str());
            let mut history = start_exec(&store, socket_id).await;
            let mut reuse = ResponsesWebSocketState::new(
                CallerKind::Bare,
                UpstreamProfile::CodexSubscription1592,
            );
            for index in 0..waits {
                let prepared = prepare_full(&store, &history, socket_id).await;
                let projected = wire(&prepared);
                assert_observations(&projected, None, index);
                let id = format!("resp_synthetic_wait_{index}");
                let output = json!([wait_call(index)]);
                if websocket {
                    let plan = reuse.plan_public_create(&projected);
                    assert_eq!(
                        plan.mode,
                        if index == 0 {
                            PublicCreateMode::Full
                        } else {
                            PublicCreateMode::Incremental
                        }
                    );
                    complete_reuse(&mut reuse, &id, &output);
                }
                let response = publish(&store, prepared, &id, output).await;
                let wait = response["output"][0].clone();
                assert_eq!(
                    wait["arguments"],
                    json!({"cell_id":RUNTIME_CELL}).to_string()
                );
                let result = wait_output(&wait["call_id"], &history[1]["call_id"]);
                history.extend([wait, result]);
            }

            let first_result = json!({"openai/resource_access":{"resources":["synthetic-a"]}});
            let changed_result = json!({"openai/resource_access":{"resources":["synthetic-b"]}});
            for (index, result) in [
                Some(first_result.clone()),
                Some(first_result),
                Some(changed_result),
                None,
            ]
            .into_iter()
            .enumerate()
            {
                let call = history[2][META]["executed_tool_calls"][0]
                    .as_object_mut()
                    .unwrap();
                match &result {
                    Some(result) => {
                        call.insert("tool_result_metadata".into(), result.clone());
                    }
                    None => {
                        call.remove("tool_result_metadata");
                    }
                }
                if index > 0 {
                    history.push(input(&format!("Synthetic follow-up {index}")));
                }
                let prepared = prepare_full(&store, &history, socket_id).await;
                let projected = wire(&prepared);
                assert_observations(&projected, result.as_ref(), waits);
                let id = format!("resp_synthetic_late_{index}");
                if websocket {
                    let plan = reuse.plan_public_create(&projected);
                    if index == 1 {
                        assert_eq!(plan.mode, PublicCreateMode::Incremental);
                        assert_eq!(plan.frame["previous_response_id"], "resp_synthetic_late_0");
                        assert_eq!(plan.frame["input"].as_array().unwrap().len(), 1);
                        assert_eq!(plan.frame["input"][0]["role"], "user");
                    } else {
                        assert_eq!(plan.mode, PublicCreateMode::Full);
                        assert!(plan.frame.get("previous_response_id").is_none());
                        assert_observations(&plan.frame, result.as_ref(), waits);
                    }
                    complete_reuse(&mut reuse, &id, &json!([]));
                }
                publish(&store, prepared, &id, json!([])).await;
            }
        }
    }
}

#[tokio::test]
async fn remote_only_wait_cannot_restore_truncated_exec_completeness() {
    let (_temp, store) = late_store();
    let socket = store.contexts.open_socket().unwrap();
    let history = start_exec(&store, Some(&socket.id)).await;
    let prepared = prepare_full(&store, &history, Some(&socket.id)).await;
    let identity = prepared.resolved_identity.clone().unwrap();
    assert_observations(&wire(&prepared), None, 0);
    let response = publish(
        &store,
        prepared,
        "resp_synthetic_remote_wait",
        json!([wait_call(0)]),
    )
    .await;
    {
        let mut inner = store.contexts.inner.lock().unwrap();
        let scope = inner
            .scopes
            .get_mut(&ContextStore::scope_key(NAMESPACE, KEY))
            .unwrap();
        let record = scope
            .records
            .get_mut(response["id"].as_str().unwrap())
            .unwrap();
        record.history = None;
        record.settings = None;
    }
    let mut next = request(json!([wait_output(
        &response["output"][0]["call_id"],
        &history[1]["call_id"],
    )]));
    next["type"] = "response.create".into();
    next["previous_response_id"] = response["id"].clone();
    let prepared = websocket_request(&store, next, &socket.id, Some(&identity))
        .await
        .unwrap();
    assert!(!prepared.rebuilt_reference);
    let projected = wire(&prepared);
    assert_eq!(
        projected["previous_response_id"],
        "resp_synthetic_remote_wait"
    );
    assert_eq!(projected["input"].as_array().unwrap().len(), 1);
    let wait = &projected["input"][0];
    assert_eq!(wait["call_id"], "call_synthetic_wait_0");
    assert_eq!(wait[META]["cell_id"], EXEC_CALL);
    assert_eq!(wait["output"], RUNTIME_CELL);
    assert!(wait[META].get("tool_calls_complete").is_none());
}
