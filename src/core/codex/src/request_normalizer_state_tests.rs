use super::*;
use crate::fingerprint::FingerprintMode;
use crate::request_state_store::RequestStateStore;
use http::HeaderValue;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;
use uuid::Uuid;

const ACCOUNT_REF: &str = "acct_stateful_normalizer";
const NAMESPACE: &str = "chatgpt-stateful-normalizer";
const SCOPE: &str = "psn_DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD";

fn store() -> (TempDir, RequestStateStore) {
    let temp = TempDir::new().expect("temp dir");
    let accounts = temp.path().join("accounts");
    fs::create_dir(&accounts).expect("accounts dir");
    (temp, RequestStateStore::new(accounts))
}

async fn prepare(
    store: &RequestStateStore,
    headers: &HeaderMap,
    body: Value,
) -> PreparedEmulatedRequest {
    prepare_stateful_subscription_request(
        EmulationTransport::Http,
        headers,
        Bytes::from(serde_json::to_vec(&body).expect("body JSON")),
        1024 * 1024,
        SubscriptionStateContext {
            account_ref: ACCOUNT_REF,
            account_namespace: NAMESPACE,
            downstream_scope: SCOPE,
            fingerprint_mode: FingerprintMode::Device,
            store,
        },
        false,
    )
    .await
    .expect("stateful preparation")
}

fn value(prepared: &PreparedEmulatedRequest) -> Value {
    serde_json::from_slice(&prepared.body).expect("prepared JSON")
}

fn turn_metadata(value: &Value) -> Value {
    serde_json::from_str(
        value["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("turn metadata"),
    )
    .expect("turn metadata JSON")
}

fn assert_uuid_version(value: &str, version: usize) {
    assert_eq!(
        Uuid::parse_str(value).expect("UUID").get_version_num(),
        version
    );
}

#[tokio::test]
async fn conflicting_root_carriers_converge_and_persist_true_uuid_versions() {
    let (_temp, store) = store();
    let mut headers = HeaderMap::new();
    headers.insert(
        "session-id",
        HeaderValue::from_static("header-session-conflict"),
    );
    headers.insert(
        "thread-id",
        HeaderValue::from_static("header-thread-conflict"),
    );
    headers.insert(
        "x-client-request-id",
        HeaderValue::from_static("header-client-conflict"),
    );
    let body = serde_json::json!({
        "model":"gpt-5.4",
        "prompt_cache_key":"cache-conflict",
        "input":[{
            "type":"message",
            "id":"msg_downstream_real",
            "role":"user",
            "content":[{"type":"input_text","text":"hello"}],
            "internal_chat_message_metadata_passthrough":{"turn_id":"turn-real"}
        }],
        "client_metadata":{
            "session_id":"body-session-canonical",
            "thread_id":"body-thread-conflict",
            "turn_id":"turn-real",
            "root_turn_id":"root-turn-conflict",
            "x-codex-installation-id":"downstream-installation",
            "x-codex-turn-metadata":"{\"session_id\":\"nested-session-conflict\",\"thread_id\":\"nested-thread-conflict\",\"turn_id\":\"turn-real\",\"root_turn_id\":\"root-turn-conflict\"}"
        }
    });

    let first = prepare(&store, &headers, body.clone()).await;
    let second = prepare(&store, &headers, body).await;
    let first_value = value(&first);
    let second_value = value(&second);
    let metadata = &first_value["client_metadata"];
    let session = metadata["session_id"].as_str().expect("session");
    let thread = metadata["thread_id"].as_str().expect("thread");
    let turn = metadata["turn_id"].as_str().expect("turn");
    let installation = metadata["x-codex-installation-id"]
        .as_str()
        .expect("installation");

    assert_eq!(session, thread);
    assert_eq!(first_value["prompt_cache_key"], session);
    assert_eq!(first.headers["session-id"], session);
    assert_eq!(first.headers["thread-id"], thread);
    assert_eq!(first.headers["x-client-request-id"], thread);
    assert_eq!(metadata["root_turn_id"], turn);
    assert_uuid_version(installation, 4);
    assert_uuid_version(session, 7);
    assert_uuid_version(turn, 7);
    assert_eq!(second_value["client_metadata"]["session_id"], session);
    assert_eq!(second_value["client_metadata"]["turn_id"], turn);
    assert_eq!(turn_metadata(&first_value)["session_id"], session);
    assert_eq!(turn_metadata(&first_value)["thread_id"], thread);
    assert_eq!(turn_metadata(&first_value)["turn_id"], turn);
    assert_ne!(first_value["input"][0]["id"], "msg_downstream_real");

    let state = fs::read_to_string(store.state_path_for_test(NAMESPACE)).expect("state file");
    for raw in [
        "body-session-canonical",
        "body-thread-conflict",
        "turn-real",
        "downstream-installation",
    ] {
        assert!(
            state.contains(raw),
            "reversible identity pair missing: {raw}"
        );
    }
    for discarded_conflict in ["cache-conflict", "root-turn-conflict"] {
        assert!(!state.contains(discarded_conflict));
    }
    assert!(state.contains("msg_downstream_real"));
    assert!(
        !state.contains("hello"),
        "request content leaked into state"
    );
}

#[tokio::test]
async fn sandbox_is_derived_from_sidecar_platform_and_header_body_stay_in_sync() {
    let (_temp, store) = store();
    let expected_sandbox = match std::env::consts::OS {
        "macos" => "seatbelt",
        "linux" | "android" => "seccomp",
        "windows" => "windows_sandbox",
        other => panic!("unsupported test platform {other}"),
    };
    let mismatched = if expected_sandbox == "seatbelt" {
        "seccomp"
    } else {
        "seatbelt"
    };
    let workspaces = serde_json::json!({
        "/downstream/workspace": {"writable_roots":["/downstream/workspace"]}
    });
    let nested = serde_json::json!({
        "session_id":"sandbox-session",
        "thread_id":"sandbox-session",
        "turn_id":"sandbox-turn",
        "request_kind":"turn",
        "sandbox_mode":"workspace-write",
        "sandbox":mismatched,
        "workspaces":workspaces.clone()
    });
    let body = serde_json::json!({
        "model":"gpt-5.4",
        "input":"hello",
        "client_metadata":{"x-codex-turn-metadata":nested.to_string()}
    });
    let prepared = prepare(&store, &HeaderMap::new(), body).await;
    let value = value(&prepared);
    let body_turn = turn_metadata(&value);
    let header_turn: Value = serde_json::from_str(
        prepared.headers["x-codex-turn-metadata"]
            .to_str()
            .expect("header metadata"),
    )
    .expect("header metadata JSON");
    for metadata in [&body_turn, &header_turn] {
        assert_eq!(metadata["sandbox_mode"], "workspace-write");
        assert_eq!(metadata["sandbox"], expected_sandbox);
        assert_eq!(metadata["workspaces"], workspaces);
    }
}

#[tokio::test]
async fn missing_turn_reuses_for_tool_roundtrip_and_changes_for_new_user() {
    let (_temp, store) = store();
    let headers = HeaderMap::new();
    let request = |input: Value| {
        serde_json::json!({
            "model":"gpt-5.4",
            "input":input,
            "client_metadata":{"session_id":"conversation-stable"}
        })
    };
    let first_input = serde_json::json!([{
        "type":"message","role":"user","content":[{"type":"input_text","text":"first"}]
    }]);
    let first = value(&prepare(&store, &headers, request(first_input.clone())).await);
    let first_turn = first["client_metadata"]["turn_id"]
        .as_str()
        .expect("first turn")
        .to_string();
    let first_user = first["input"]
        .as_array()
        .expect("input")
        .iter()
        .find(|item| item["role"] == "user")
        .expect("user");
    let first_item_id = first_user["id"].as_str().expect("item id").to_string();
    let first_create_time =
        first_user["internal_chat_message_metadata_passthrough"]["create_time"].clone();

    let tool_input = serde_json::json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]},
        {"type":"function_call_output","call_id":"call_downstream","output":"done"}
    ]);
    let tool = value(&prepare(&store, &headers, request(tool_input)).await);
    assert_eq!(tool["client_metadata"]["turn_id"], first_turn);
    let repeated_user = tool["input"]
        .as_array()
        .expect("input")
        .iter()
        .find(|item| item["role"] == "user")
        .expect("user");
    assert_eq!(repeated_user["id"], first_item_id);
    assert_eq!(
        repeated_user["internal_chat_message_metadata_passthrough"]["create_time"],
        first_create_time
    );

    let minimal_tool = value(
        &prepare(
            &store,
            &headers,
            request(serde_json::json!([{
                "type":"function_call_output",
                "call_id":"call_downstream_minimal",
                "output":"done"
            }])),
        )
        .await,
    );
    assert_eq!(minimal_tool["client_metadata"]["turn_id"], first_turn);

    let next_input = serde_json::json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]},
        {"type":"function_call_output","call_id":"call_downstream","output":"done"},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"second"}]}
    ]);
    let next = value(&prepare(&store, &headers, request(next_input)).await);
    assert_ne!(next["client_metadata"]["turn_id"], first_turn);
}

#[tokio::test]
async fn explicit_parent_lineage_keeps_root_session_and_distinct_child_thread() {
    let (_temp, store) = store();
    let body = serde_json::json!({
        "model":"gpt-5.4",
        "input":[{"type":"message","role":"user","content":"child work"}],
        "client_metadata":{
            "session_id":"root-session",
            "thread_id":"child-thread",
            "x-codex-turn-metadata":"{\"parent_thread_id\":\"root-session\",\"turn_id\":\"child-turn\",\"root_turn_id\":\"root-turn\"}"
        }
    });
    let prepared = prepare(&store, &HeaderMap::new(), body).await;
    let value = value(&prepared);
    let metadata = &value["client_metadata"];
    assert_ne!(metadata["session_id"], metadata["thread_id"]);
    assert_eq!(metadata["parent_thread_id"], metadata["session_id"],);
    assert_ne!(metadata["turn_id"], metadata["root_turn_id"]);
    assert_eq!(
        prepared.headers["x-codex-parent-thread-id"],
        metadata["session_id"].as_str().expect("session")
    );
}

#[tokio::test]
async fn oversized_schema_id_is_an_invalid_request_not_a_state_outage() {
    let (_temp, store) = store();
    let body = serde_json::json!({
        "model":"gpt-5.4",
        "input":"hello",
        "client_metadata":{"session_id":"x".repeat(513)}
    });
    let error = prepare_stateful_subscription_request(
        EmulationTransport::Http,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).expect("body JSON")),
        1024 * 1024,
        SubscriptionStateContext {
            account_ref: ACCOUNT_REF,
            account_namespace: NAMESPACE,
            downstream_scope: SCOPE,
            fingerprint_mode: FingerprintMode::Device,
            store: &store,
        },
        false,
    )
    .await
    .expect_err("oversized ID must fail");
    assert_eq!(error, StatefulPrepareError::InvalidRequest);
    assert!(!store.state_path_for_test(NAMESPACE).exists());
}
