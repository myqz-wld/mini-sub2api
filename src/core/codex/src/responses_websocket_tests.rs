use super::*;
use crate::request_state_types::WireIdDomain;
use crate::server::internal_router;
use crate::test_support::spawn_loopback;
use crate::upstream_request::websocket_url;
use crate::vault::Vault;
use axum::Router;
use axum::extract::State as AxumState;
use axum::response::Response as AxumResponse;
use axum::routing::get;
use pretty_assertions::assert_eq;
use reqwest_websocket::CloseCode as DownstreamCloseCode;
use reqwest_websocket::Message as DownstreamMessage;
use reqwest_websocket::RequestBuilderExt;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const INTERNAL_TOKEN: &str = "internal-websocket-test-token-at-least-32-bytes";
const ACCOUNT_NAMESPACE: &str = "chatgpt-account-test";
const ACCOUNT_REF: &str = "acct_websocket_prepare";
const PSEUDONYM_SCOPE: &str = "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[path = "responses_websocket_context_tests.rs"]
mod context_tests;
#[path = "responses_websocket_inject_tests.rs"]
mod inject_tests;

fn device_fingerprint() -> FingerprintSnapshot {
    FingerprintSnapshot::for_test(FingerprintMode::Device, 1)
}

#[test]
fn converts_only_supported_upstream_url_schemes() {
    assert_eq!(
        websocket_url("https://api.openai.com/v1/responses?trace=1")
            .expect("HTTPS URL")
            .as_str(),
        "wss://api.openai.com/v1/responses?trace=1"
    );
    assert_eq!(
        websocket_url("http://127.0.0.1:1234/responses")
            .expect("HTTP URL")
            .as_str(),
        "ws://127.0.0.1:1234/responses"
    );
    assert!(websocket_url("file:///tmp/responses").is_err());
}

fn request_state_store() -> (
    tempfile::TempDir,
    crate::request_state_store::RequestStateStore,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = crate::request_state_store::RequestStateStore::new(temp.path().to_path_buf());
    (temp, store)
}

#[tokio::test]
async fn bare_api_key_create_frame_is_byte_exact_and_never_uses_state() {
    let original = " {\"type\":\"response.create\", \"model\":\"test\"} ".to_string();
    let mut headers = HeaderMap::new();
    let (_temp, store) = request_state_store();
    let mut identity = None;
    let got = prepare_client_text(
        original.clone(),
        &mut headers,
        ACCOUNT_REF,
        None,
        UpstreamProfile::ApiKeyPassthrough,
        PSEUDONYM_SCOPE,
        &device_fingerprint(),
        &store,
        &mut identity,
    )
    .await
    .expect("valid frame");
    assert_eq!(got.text, original);
    assert!(!store.state_path_for_test(ACCOUNT_REF).exists());
}

#[tokio::test]
async fn subscription_create_normalization_preserves_websocket_fields() {
    let original = serde_json::json!({
        "type": "response.create",
        "model": "gpt-5.6-sol",
        "input": "hello",
        "tools": [],
        "generate": false,
        "stream_id": "stream-caller",
        "background": true,
        "stream": true,
        "client_metadata": {"custom": "kept"}
    })
    .to_string();
    let mut headers = HeaderMap::new();
    let (_temp, store) = request_state_store();
    let mut identity = None;
    let got = prepare_client_text(
        original,
        &mut headers,
        ACCOUNT_REF,
        Some(ACCOUNT_NAMESPACE),
        UpstreamProfile::CodexSubscription1560,
        PSEUDONYM_SCOPE,
        &device_fingerprint(),
        &store,
        &mut identity,
    )
    .await
    .expect("valid frame");
    let value: Value = serde_json::from_str(&got.text).expect("normalized JSON");

    assert_eq!(value["type"], "response.create");
    assert_eq!(value["generate"], false);
    assert_ne!(value["stream_id"], "stream-caller");
    assert!(value.get("background").is_none());
    assert_eq!(value["stream"], true);
    assert!(value.get("previous_response_id").is_none());
    assert_eq!(value["client_metadata"]["custom"], "kept");
    assert_eq!(value["input"][0]["type"], "additional_tools");
    assert_eq!(value["store"], false);
}

#[tokio::test]
async fn valid_non_create_application_event_is_byte_exact() {
    let original = "{\"type\":\"response.append_input_item\",\"item\":{}}".to_string();
    let mut headers = HeaderMap::new();
    let (_temp, store) = request_state_store();
    let mut identity = None;
    let got = prepare_client_text(
        original.clone(),
        &mut headers,
        ACCOUNT_REF,
        None,
        UpstreamProfile::ApiKeyPassthrough,
        PSEUDONYM_SCOPE,
        &device_fingerprint(),
        &store,
        &mut identity,
    )
    .await
    .expect("valid control frame");
    assert_eq!(got.text, original);
}

async fn bind_control_test_operation(
    store: &crate::request_state_store::RequestStateStore,
    call_alias: &str,
    identity: &mut Option<ResolvedRequestIdentity>,
) -> crate::response_translation::ResponseStateContext {
    let prepared = prepare_client_text(
        serde_json::json!({"type":"response.create","model":"gpt-5.4","instructions":"base",
        "input":[{"type":"function_call","call_id":call_alias,"name":"example","arguments":"{}"}]})
        .to_string(),
        &mut HeaderMap::new(),
        ACCOUNT_REF,
        Some(ACCOUNT_NAMESPACE),
        UpstreamProfile::CodexSubscription1560,
        PSEUDONYM_SCOPE,
        &device_fingerprint(),
        store,
        identity,
    )
    .await
    .expect("bound create");
    let response = crate::response_translation::ResponseStateContext::new(
        ACCOUNT_REF,
        ACCOUNT_NAMESPACE,
        PSEUDONYM_SCOPE,
        store,
        identity.as_ref(),
        None,
    )
    .with_operation(prepared.operation);
    response
        .translate_value(
            serde_json::json!({"type":"response.created","response":{"id":"resp_provider"}}),
        )
        .await
        .expect("created ownership");
    response
}

#[tokio::test]
async fn subscription_control_frame_ids_are_stable_and_pseudonymized() {
    let (_temp, store) = request_state_store();
    let (response_alias, call_alias) = store
        .edit(ACCOUNT_NAMESPACE, ACCOUNT_REF, PSEUDONYM_SCOPE, |editor| {
            Ok((
                editor.wire_from_upstream(WireIdDomain::Response, "resp_provider")?,
                editor.wire_from_upstream(WireIdDomain::Call, "call_provider")?,
            ))
        })
        .await
        .expect("seed control references");
    let original = serde_json::json!({
        "type":"response.append_input_item",
        "response_id":response_alias,
        "item":{
            "type":"function_call_output",
            "id":"item_downstream",
            "call_id":call_alias,
            "output":{"opaque_id":"opaque_keep"}
        }
    })
    .to_string();
    let mut identity = None;
    let _operation = bind_control_test_operation(&store, &call_alias, &mut identity).await;
    let mut first_headers = HeaderMap::new();
    let first = prepare_client_text(
        original.clone(),
        &mut first_headers,
        ACCOUNT_REF,
        Some(ACCOUNT_NAMESPACE),
        UpstreamProfile::CodexSubscription1560,
        PSEUDONYM_SCOPE,
        &device_fingerprint(),
        &store,
        &mut identity,
    )
    .await
    .expect("first control frame");
    let mut second_headers = HeaderMap::new();
    let second = prepare_client_text(
        original,
        &mut second_headers,
        ACCOUNT_REF,
        Some(ACCOUNT_NAMESPACE),
        UpstreamProfile::CodexSubscription1560,
        PSEUDONYM_SCOPE,
        &device_fingerprint(),
        &store,
        &mut identity,
    )
    .await
    .expect("second control frame");
    assert_eq!(first.text, second.text);
    let value: Value = serde_json::from_str(&first.text).expect("control JSON");
    assert_eq!(value["response_id"], "resp_provider");
    assert_ne!(value["item"]["id"], "item_downstream");
    assert_eq!(value["item"]["call_id"], "call_provider");
    assert_eq!(value["item"]["output"]["opaque_id"], "opaque_keep");
}

#[tokio::test]
async fn malformed_or_untyped_application_frames_are_rejected() {
    let (_temp, store) = request_state_store();
    for frame in ["not-json", "{}", r#"{"type":""}"#] {
        let mut headers = HeaderMap::new();
        let mut identity = None;
        assert!(
            prepare_client_text(
                frame.to_string(),
                &mut headers,
                ACCOUNT_REF,
                None,
                UpstreamProfile::ApiKeyPassthrough,
                PSEUDONYM_SCOPE,
                &device_fingerprint(),
                &store,
                &mut identity,
            )
            .await
            .is_err(),
            "frame {frame}"
        );
    }
}

#[derive(Clone, Default)]
struct WebSocketCapture {
    headers: Arc<Mutex<Option<HeaderMap>>>,
    frames: Arc<Mutex<Vec<String>>>,
    calls: Arc<AtomicUsize>,
}

struct RunningInternalServer {
    base_url: String,
    task: JoinHandle<()>,
}

impl Drop for RunningInternalServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[path = "responses_websocket_transport_tests.rs"]
mod transport_tests;

#[path = "responses_websocket_test_support.rs"]
mod test_support;
use test_support::*;

#[path = "responses_websocket_oauth_tests.rs"]
mod oauth_tests;

#[path = "responses_websocket_delivery_tests.rs"]
mod delivery_tests;

#[path = "responses_websocket_deferred_oauth_tests.rs"]
mod deferred_oauth_tests;

#[path = "responses_websocket_policy_tests.rs"]
mod policy_tests;

#[path = "responses_websocket_size_tests.rs"]
mod size_tests;

#[path = "responses_websocket_initial_tests.rs"]
mod initial_tests;

#[path = "responses_websocket_diagnostics_tests.rs"]
mod diagnostics_tests;

#[path = "responses_websocket_reference_tests.rs"]
mod reference_tests;

#[path = "responses_websocket_passthrough_tests.rs"]
mod passthrough_tests;
