use crate::fingerprint::FingerprintMode;
use crate::request_normalizer::{
    CodexStateContext, EmulationTransport, PreparedEmulatedRequest, prepare_stateful_codex_request,
};
use crate::request_profile::UpstreamProfile;
use crate::request_state_store::RequestStateStore;
use crate::response_translation::ResponseStateContext;
use crate::tool_observation_budget::{MESSAGE_BYTES, META};
use bytes::Bytes;
use http::HeaderMap;
use serde_json::{Value, json};

const NS: &str = "synthetic-state-audit";
const OWNER: &str = "acct_state_audit";
const KEY: &str = "synthetic-key";

async fn prepare(store: &RequestStateStore, body: Value) -> PreparedEmulatedRequest {
    prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription1592,
        EmulationTransport::Http,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        128 * 1024 * 1024,
        CodexStateContext {
            force_lite: false,
            admission: None,
            store,
            state_namespace: NS,
            account_ref: OWNER,
            downstream_scope: KEY,
            fingerprint_mode: FingerprintMode::Device,
            binding: None,
            socket_id: None,
        },
        false,
    )
    .await
    .expect("synthetic full request accepted")
}

async fn publish(store: &RequestStateStore, prepared: PreparedEmulatedRequest) {
    let state = ResponseStateContext::new(
        OWNER,
        NS,
        KEY,
        store,
        prepared.resolved_identity.as_ref(),
        None,
    )
    .with_operation(prepared.operation);
    state
        .translate_value(json!({"type":"response.created","response":{"id":"resp_synthetic_loss"}}))
        .await
        .unwrap();
    state.translate_value(json!({"type":"response.completed","response":{"id":"resp_synthetic_loss","output":[]}}))
        .await.unwrap();
}

fn request() -> Value {
    let arguments = json!({"padding":"a".repeat(4096)});
    json!({"model":"gpt-5.4","instructions":"synthetic base",
    "client_metadata":{"session_id":"source-session","turn_id":"source-turn"},
    "input":[
        {"type":"message","role":"user","content":[{"type":"input_text","text":"synthetic request"}]},
        {"type":"function_call","call_id":"call_synthetic","name":"probe","arguments":arguments.to_string()},
        {"type":"function_call_output","id":"fco_synthetic","call_id":"call_synthetic","output":"synthetic result",
         META:{"executed_tool_calls":[{"name":"probe","arguments":arguments}],"tool_calls_complete":true}}
    ]})
}

fn output(body: &[u8]) -> Value {
    let body: Value = serde_json::from_slice(body).unwrap();
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["type"] == "function_call_output" || i["type"] == "custom_tool_call_output")
        .unwrap()
        .clone()
}

fn cell_request() -> Value {
    let mut body = request();
    body["input"][1] = json!({"type":"custom_tool_call","call_id":"call_synthetic","name":"exec",
        "input":format!("await tools.probe({{padding:{}}})", json!("a".repeat(4096)))});
    body["input"][2]["type"] = "custom_tool_call_output".into();
    body["input"][2]["id"] = "ctco_synthetic".into();
    body["input"][2][META]["cell_id"] = "call_synthetic".into();
    body
}

#[tokio::test]
async fn optional_output_id_omission_retains_revocation_after_wire_loss() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let mut large = request();
    large["instructions"] = "b".repeat(MESSAGE_BYTES - 8192).into();
    let first = prepare(&store, large).await;
    let original = first.resolved_identity.as_ref().unwrap().clone();
    let item = output(&first.body);
    assert!(
        first.body.len() <= MESSAGE_BYTES,
        "business content fits the soft limit"
    );
    assert!(
        item[META].get("tool_calls_complete").is_none(),
        "first wire inventory was shed"
    );
    assert!(
        item[META].get("executed_tool_calls").is_none()
            || item[META]["executed_tool_calls"][0]["arguments"]
                .get("_codex_executed_tool_call_truncated")
                .is_some()
    );
    publish(&store, first).await;
    let control = prepare(&store, request()).await;
    assert!(
        output(&control.body)[META]
            .get("tool_calls_complete")
            .is_none(),
        "unchanged output ID retains its revocation"
    );
    drop(control);
    let mut replay = request();
    replay["input"][2].as_object_mut().unwrap().remove("id");
    let second = prepare(&store, replay).await;
    assert_eq!(
        second.resolved_identity.as_ref().unwrap().thread_id,
        original.thread_id
    );
    assert!(
        output(&second.body)[META]
            .get("tool_calls_complete")
            .is_none(),
        "omitting an optional item id must preserve the call revocation"
    );
}

#[tokio::test]
async fn detached_history_import_retains_revocation_after_wire_loss() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let mut large = cell_request();
    large["instructions"] = "b".repeat(MESSAGE_BYTES - 8192).into();
    let first = prepare(&store, large).await;
    let original = first.resolved_identity.as_ref().unwrap().clone();
    assert!(
        first.body.len() <= MESSAGE_BYTES,
        "business content fits the soft limit"
    );
    assert!(
        output(&first.body)[META]
            .get("tool_calls_complete")
            .is_none()
    );
    publish(&store, first).await;
    drop(store);
    let store = RequestStateStore::new(temp.path().into());
    let control = prepare(&store, cell_request()).await;
    assert!(
        output(&control.body)[META]
            .get("tool_calls_complete")
            .is_none(),
        "unchanged source-thread ownership retains its revocation after restart"
    );
    drop(control);
    let mut replay = cell_request();
    replay.as_object_mut().unwrap().remove("client_metadata");
    for item in replay["input"].as_array_mut().unwrap() {
        if item.get(META).is_none() {
            item[META] = json!({});
        }
        item[META]["turn_id"] = "source-turn".into();
    }
    let second = prepare(&store, replay).await;
    assert_ne!(
        second.resolved_identity.as_ref().unwrap().thread_id,
        original.thread_id
    );
    let item = output(&second.body);
    assert_ne!(item[META]["turn_id"].as_str(), original.turn_id.as_deref());
    assert!(
        item[META].get("tool_calls_complete").is_none(),
        "an imported turn copy must preserve the original source-thread revocation"
    );
}

#[tokio::test]
async fn code_mode_cell_origin_is_projected_with_its_provider_call() {
    let temp = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temp.path().into());
    let mut first = request();
    first["input"] = json!([first["input"][0]]);
    let first = prepare(&store, first).await;
    let state = ResponseStateContext::new(
        OWNER,
        NS,
        KEY,
        &store,
        first.resolved_identity.as_ref(),
        None,
    )
    .with_operation(first.operation);
    state
        .translate_value(json!({"type":"response.created","response":{"id":"resp_synthetic_cell"}}))
        .await
        .unwrap();
    let completed = state
        .translate_value(json!({"type":"response.completed","response":{
        "id":"resp_synthetic_cell", "output":[{"type":"custom_tool_call","id":"ctc_provider",
        "call_id":"call_provider","name":"exec","input":"await tools.probe({})"},
        {"type":"function_call","id":"fc_provider_wait","call_id":"call_provider_wait",
         "name":"wait","arguments":"{\"cell_id\":\"runtime_handle_opaque\"}"}]}}))
        .await
        .unwrap();
    let caller_call = completed["response"]["output"][0]["call_id"].clone();
    let caller_wait = completed["response"]["output"][1]["call_id"].clone();
    assert_ne!(caller_call, "call_provider");
    let next = json!({"model":"gpt-5.4","previous_response_id":completed["response"]["id"],
        "input":[{"type":"custom_tool_call_output","call_id":caller_call,"output":"synthetic result",
        META:{"cell_id":caller_call,"executed_tool_calls":[{"name":"tools.probe","arguments":{}}],
        "tool_calls_complete":true}},
        {"type":"function_call_output","call_id":caller_wait,"output":"runtime_handle_opaque",
         META:{"cell_id":caller_call,"tool_calls_complete":true}}]});
    let prepared = prepare(&store, next).await;
    let item = output(&prepared.body);
    assert_eq!(item["call_id"], "call_provider");
    assert_eq!(
        item[META]["cell_id"], item["call_id"],
        "originating exec call and cell attribution must use the same ID domain"
    );
    let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
    let wait = wire["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap();
    assert_eq!(wait["call_id"], "call_provider_wait");
    assert_eq!(wait[META]["cell_id"], "call_provider");
    assert_eq!(wait["output"], "runtime_handle_opaque");
    assert_eq!(
        completed["response"]["output"][1]["arguments"],
        "{\"cell_id\":\"runtime_handle_opaque\"}"
    );
    drop(prepared);

    for cell in [json!("unbound-origin"), json!("x".repeat(513)), json!(42)] {
        let prepared = prepare(
            &store,
            json!({"model":"gpt-5.4",
            "previous_response_id":completed["response"]["id"],
            "input":[{"type":"custom_tool_call_output","call_id":caller_call,"output":"kept",
            META:{"cell_id":cell,"tool_calls_complete":true}}]}),
        )
        .await;
        let item = output(&prepared.body);
        assert_eq!(item["output"], "kept");
        assert!(item[META].get("cell_id").is_none());
        assert!(item[META].get("tool_calls_complete").is_none());
    }
}
