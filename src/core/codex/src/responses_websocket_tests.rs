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
        UpstreamProfile::BareOpenAi,
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
        UpstreamProfile::CodexSubscription149,
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
    assert!(value.get("stream").is_none());
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
        Some(ACCOUNT_NAMESPACE),
        UpstreamProfile::CodexSubscription149,
        PSEUDONYM_SCOPE,
        &device_fingerprint(),
        &store,
        &mut identity,
    )
    .await
    .expect("valid control frame");
    assert_eq!(got.text, original);
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
    let mut first_headers = HeaderMap::new();
    let first = prepare_client_text(
        original.clone(),
        &mut first_headers,
        ACCOUNT_REF,
        Some(ACCOUNT_NAMESPACE),
        UpstreamProfile::CodexSubscription149,
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
        UpstreamProfile::CodexSubscription149,
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
                UpstreamProfile::BareOpenAi,
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

#[tokio::test]
async fn bare_api_key_route_relays_byte_exact_turns_and_filters_handshake_headers() {
    let capture = WebSocketCapture::default();
    let app = Router::new()
        .route("/responses", get(accepting_upstream))
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let vault = state.vault.clone();
    let core = spawn_internal(state).await;

    let handshake = internal_handshake(&core.base_url, &account_ref)
        .header("user-agent", "OpenAI/Go websocket-test")
        .header("openai-beta", "must-be-replaced")
        .header("openai-organization", "org-test")
        .header("cookie", "must-not-cross=1")
        .header("x-forwarded-for", "203.0.113.20")
        .upgrade()
        .send()
        .await
        .expect("internal handshake");

    assert_eq!(handshake.status(), StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(
        handshake.headers().get("x-models-etag").unwrap(),
        "etag-test"
    );
    assert!(!handshake.headers().contains_key("set-cookie"));
    assert!(!handshake.headers().contains_key("x-upstream-private"));
    assert!(handshake.headers().contains_key(CORE_TTFB_HEADER));
    let mut socket = handshake.into_websocket().await.expect("internal socket");

    let first = " {\"type\":\"response.create\", \"model\":\"first\"} ";
    let second =
        r#"{"type":"response.create","model":"second","previous_response_id":"resp_first"}"#;
    for (index, frame) in [first, second].into_iter().enumerate() {
        socket
            .send(DownstreamMessage::Text(frame.to_string()))
            .await
            .expect("send create frame");
        let event = socket
            .next()
            .await
            .expect("completion event")
            .expect("valid completion event");
        let DownstreamMessage::Text(event) = event else {
            panic!("expected text completion event");
        };
        let value: Value = serde_json::from_str(&event).expect("event JSON");
        assert_eq!(value["sequence"], index + 1);
    }
    let _ = socket.close(DownstreamCloseCode::Normal, None).await;

    assert_eq!(capture.frames.lock().await.as_slice(), [first, second]);
    assert!(
        !vault
            .request_state()
            .state_path_for_test(&account_ref)
            .exists(),
        "BareOpenAi WebSocket created request state"
    );
    let headers = capture
        .headers
        .lock()
        .await
        .clone()
        .expect("upstream headers");
    assert_eq!(
        header_text(&headers, http::header::AUTHORIZATION.as_str()).as_deref(),
        Some("Bearer upstream-websocket-api-key-test")
    );
    assert_eq!(
        header_text(&headers, "openai-beta").as_deref(),
        Some(crate::upstream_request::RESPONSES_WEBSOCKET_BETA)
    );
    assert_eq!(
        header_text(&headers, "openai-organization").as_deref(),
        Some("org-test")
    );
    assert_eq!(
        header_text(&headers, "sec-websocket-extensions").as_deref(),
        Some("permessage-deflate; client_max_window_bits")
    );
    for forbidden in [
        "cookie",
        "x-forwarded-for",
        mini_sub2api_protocol_v1::ACCOUNT_REF_HEADER,
    ] {
        assert!(!headers.contains_key(forbidden), "header {forbidden}");
    }
}

#[tokio::test]
async fn codex_api_key_defers_provider_handshake_and_reuses_state_after_reconnect() {
    let capture = WebSocketCapture::default();
    let app = Router::new()
        .route("/responses", get(accepting_upstream))
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let vault = state.vault.clone();
    let core = spawn_internal(state).await;
    let create = serde_json::json!({
        "type":"response.create",
        "model":"gpt-5.4",
        "input":[{"type":"message","id":"msg_down","role":"user","content":"hello"}],
        "client_metadata":{
            "session_id":"session_down",
            "thread_id":"thread_down",
            "turn_id":"turn_down"
        }
    })
    .to_string();

    let mut first = internal_handshake(&core.base_url, &account_ref)
        .header("originator", "codex_exec")
        .upgrade()
        .send()
        .await
        .expect("first internal handshake")
        .into_websocket()
        .await
        .expect("first internal socket");
    assert_eq!(capture.calls.load(Ordering::SeqCst), 0);
    first
        .send(DownstreamMessage::Text(create.clone()))
        .await
        .expect("first create");
    let first_event = first
        .next()
        .await
        .expect("first completion")
        .expect("first completion event");
    let DownstreamMessage::Text(first_event) = first_event else {
        panic!("expected first completion text")
    };
    let first_event: Value = serde_json::from_str(&first_event).expect("first completion JSON");
    let first_response_id = first_event["response"]["id"]
        .as_str()
        .expect("first response alias")
        .to_string();
    assert_ne!(first_response_id, "resp_provider");
    assert_eq!(capture.calls.load(Ordering::SeqCst), 1);
    let first_frame: Value =
        serde_json::from_str(&capture.frames.lock().await[0]).expect("first projected frame");
    let first_metadata = &first_frame["client_metadata"];
    for (name, raw, version) in [
        ("session_id", "session_down", 7),
        ("thread_id", "thread_down", 7),
        ("turn_id", "turn_down", 7),
        (
            "x-codex-installation-id",
            "00000000-0000-0000-0000-000000000000",
            4,
        ),
    ] {
        let projected = first_metadata[name].as_str().expect("projected identity");
        assert_ne!(projected, raw);
        assert_eq!(
            uuid::Uuid::parse_str(projected)
                .expect("projected UUID")
                .get_version_num(),
            version
        );
    }
    let projected_session = first_metadata["session_id"]
        .as_str()
        .expect("projected session")
        .to_string();
    assert!(
        vault
            .request_state()
            .state_path_for_test(&account_ref)
            .is_file()
    );
    first
        .close(DownstreamCloseCode::Normal, None)
        .await
        .expect("close first socket");

    let mut second = internal_handshake(&core.base_url, &account_ref)
        .header("originator", "codex_exec")
        .upgrade()
        .send()
        .await
        .expect("second internal handshake")
        .into_websocket()
        .await
        .expect("second internal socket");
    assert_eq!(capture.calls.load(Ordering::SeqCst), 1);
    second
        .send(DownstreamMessage::Text(create))
        .await
        .expect("second create");
    let second_event = second
        .next()
        .await
        .expect("second completion")
        .expect("second completion event");
    let DownstreamMessage::Text(second_event) = second_event else {
        panic!("expected second completion text")
    };
    let second_event: Value = serde_json::from_str(&second_event).expect("second completion JSON");
    assert_eq!(second_event["response"]["id"], first_response_id);
    assert_eq!(capture.calls.load(Ordering::SeqCst), 2);
    let second_frame: Value =
        serde_json::from_str(&capture.frames.lock().await[1]).expect("second projected frame");
    assert_eq!(
        second_frame["client_metadata"]["session_id"],
        projected_session
    );
}

#[tokio::test]
async fn codex_api_key_websocket_compaction_commits_only_completed_terminal() {
    let capture = WebSocketCapture::default();
    let app = Router::new()
        .route("/responses", get(compaction_upstream))
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let core = spawn_internal(state).await;
    let mut socket = internal_handshake(&core.base_url, &account_ref)
        .header("originator", "codex_exec")
        .upgrade()
        .send()
        .await
        .expect("internal handshake")
        .into_websocket()
        .await
        .expect("internal socket");
    let compaction = |model: &str, turn: &str| {
        let metadata = serde_json::json!({
            "session_id":"compaction-session",
            "thread_id":"compaction-session",
            "turn_id":turn,
            "request_kind":"compaction",
            "compaction":{"trigger":"manual","implementation":"responses_compaction_v2"}
        });
        serde_json::json!({
            "type":"response.create",
            "model":model,
            "input":[{"type":"compaction_trigger"}],
            "client_metadata":{"x-codex-turn-metadata":metadata.to_string()}
        })
        .to_string()
    };
    for frame in [
        compaction("fail-compaction", "turn-one"),
        compaction("complete-compaction", "turn-one"),
        compaction("complete-compaction", "turn-two"),
    ] {
        socket
            .send(DownstreamMessage::Text(frame))
            .await
            .expect("send compaction");
        let event = socket
            .next()
            .await
            .expect("terminal event")
            .expect("valid terminal event");
        assert!(matches!(event, DownstreamMessage::Text(_)));
    }
    let frames = capture.frames.lock().await;
    let windows = frames
        .iter()
        .map(|frame| {
            serde_json::from_str::<Value>(frame)
                .expect("projected compaction")
                .get("client_metadata")
                .and_then(|metadata| metadata.get("x-codex-window-id"))
                .and_then(Value::as_str)
                .expect("projected compaction window")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert!(windows[0].ends_with(":0"));
    assert_eq!(windows[1], windows[0], "failed terminal advanced window");
    assert!(windows[2].ends_with(":1"));
}

#[tokio::test]
async fn upstream_rejection_stays_http_and_is_bounded_to_safe_metadata() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/responses",
        get({
            let calls = Arc::clone(&calls);
            move || {
                let calls = Arc::clone(&calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::UPGRADE_REQUIRED,
                        [
                            ("content-type", "application/json"),
                            ("set-cookie", "must-not-cross=1"),
                        ],
                        r#"{"error":{"code":"websocket_required"}}"#,
                    )
                }
            }
        }),
    );
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let core = spawn_internal(state).await;

    let rejection = internal_handshake(&core.base_url, &account_ref)
        .upgrade()
        .send()
        .await
        .expect("rejected internal handshake");
    assert_eq!(rejection.status(), StatusCode::UPGRADE_REQUIRED);
    assert!(!rejection.headers().contains_key("set-cookie"));
    let body = rejection.into_inner().text().await.expect("rejection body");
    assert_eq!(body, r#"{"error":{"code":"websocket_required"}}"#);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn invalid_internal_auth_never_reaches_upstream() {
    let capture = WebSocketCapture::default();
    let app = Router::new()
        .route("/responses", get(accepting_upstream))
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let core = spawn_internal(state).await;

    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client")
        .get(format!("{}/internal/v1/responses/ws", core.base_url))
        .header(http::header::AUTHORIZATION, "Bearer wrong-internal-token")
        .header(
            mini_sub2api_protocol_v1::VERSION_HEADER,
            mini_sub2api_protocol_v1::VERSION,
        )
        .header(mini_sub2api_protocol_v1::ACCOUNT_REF_HEADER, account_ref)
        .header(mini_sub2api_protocol_v1::REQUEST_ID_HEADER, "req_ws_test")
        .upgrade()
        .send()
        .await
        .expect("auth rejection");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(capture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn loopback_websocket_uses_direct_client_and_enforces_message_limit() {
    let capture = WebSocketCapture::default();
    let app = Router::new()
        .route("/responses", get(accepting_upstream))
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let (mut state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    state.transports = Arc::new(
        crate::transport_registry::TransportRegistry::new_with_proxy_url("http://127.0.0.1:1")
            .expect("proxied transport registry"),
    );
    let core = spawn_internal(state).await;
    let handshake = internal_handshake(&core.base_url, &account_ref)
        .upgrade()
        .send()
        .await
        .expect("direct loopback handshake");
    let mut socket = handshake.into_websocket().await.expect("internal socket");
    let oversized = format!(
        "{{\"type\":\"response.create\",\"padding\":\"{}\"}}",
        "a".repeat(MAX_WEBSOCKET_MESSAGE_BYTES)
    );
    let send_result = socket.send(DownstreamMessage::Text(oversized)).await;
    if send_result.is_ok() {
        let closed = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("oversized close timeout");
        assert!(!matches!(closed, Some(Ok(DownstreamMessage::Text(_)))));
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(capture.frames.lock().await.is_empty());
}

async fn accepting_upstream(
    AxumState(capture): AxumState<WebSocketCapture>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    capture.calls.fetch_add(1, Ordering::SeqCst);
    *capture.headers.lock().await = Some(headers);
    let relay_capture = capture.clone();
    let mut response = upgrade
        .max_message_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .max_frame_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .on_upgrade(move |mut socket| async move {
            let mut sequence = 0;
            while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
                sequence += 1;
                relay_capture.frames.lock().await.push(frame.to_string());
                let event = serde_json::json!({
                    "type": "response.completed",
                    "sequence": sequence,
                    "response": {
                        "id":"resp_provider",
                        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                    }
                })
                .to_string();
                if socket
                    .send(InternalMessage::Text(event.into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        })
        .into_response();
    response
        .headers_mut()
        .insert("x-models-etag", HeaderValue::from_static("etag-test"));
    response
        .headers_mut()
        .insert("set-cookie", HeaderValue::from_static("must-not-cross=1"));
    response.headers_mut().insert(
        "x-upstream-private",
        HeaderValue::from_static("must-not-cross"),
    );
    response
}

async fn compaction_upstream(
    AxumState(capture): AxumState<WebSocketCapture>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    capture.calls.fetch_add(1, Ordering::SeqCst);
    *capture.headers.lock().await = Some(headers);
    let relay_capture = capture.clone();
    upgrade
        .max_message_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .max_frame_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .on_upgrade(move |mut socket| async move {
            while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
                relay_capture.frames.lock().await.push(frame.to_string());
                let value: Value = serde_json::from_str(&frame).expect("compaction frame JSON");
                let completed = value["model"] == "complete-compaction";
                let event = serde_json::json!({
                    "type": if completed { "response.completed" } else { "response.failed" },
                    "response":{"id": if completed { "resp_completed" } else { "resp_failed" }}
                })
                .to_string();
                if socket
                    .send(InternalMessage::Text(event.into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        })
        .into_response()
}

async fn api_key_state(base_url: &str) -> (AppState, String, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-websocket-api-key-test".to_string(),
            format!("{base_url}/responses"),
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("API key record");
    let state = app_state(vault);
    (state, metadata.account_ref, temp)
}

fn app_state(vault: Vault) -> AppState {
    AppState {
        vault,
        transports: Arc::new(
            crate::transport_registry::TransportRegistry::new().expect("transport registry"),
        ),
        internal_token_hash: Sha256::digest(INTERNAL_TOKEN.as_bytes()).into(),
        account_locks: Arc::new(Mutex::new(HashMap::new())),
    }
}

async fn spawn_internal(state: AppState) -> RunningInternalServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind internal test server");
    let address = listener.local_addr().expect("internal test address");
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            internal_router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve internal test server");
    });
    RunningInternalServer {
        base_url: format!("http://{address}"),
        task,
    }
}

fn internal_handshake(base_url: &str, account_ref: &str) -> reqwest::RequestBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("internal client")
        .get(format!("{base_url}/internal/v1/responses/ws"))
        .header(
            http::header::AUTHORIZATION,
            format!("Bearer {INTERNAL_TOKEN}"),
        )
        .header(
            mini_sub2api_protocol_v1::VERSION_HEADER,
            mini_sub2api_protocol_v1::VERSION,
        )
        .header(mini_sub2api_protocol_v1::ACCOUNT_REF_HEADER, account_ref)
        .header(
            mini_sub2api_protocol_v1::PSEUDONYM_SCOPE_HEADER,
            PSEUDONYM_SCOPE,
        )
        .header(mini_sub2api_protocol_v1::REQUEST_ID_HEADER, "req_ws_test")
}

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
