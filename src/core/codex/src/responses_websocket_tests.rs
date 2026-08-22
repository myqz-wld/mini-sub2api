use super::*;
use crate::server::internal_router;
use crate::test_support::spawn_loopback;
use crate::upstream_request::websocket_url;
use crate::vault::Vault;
use axum::Router;
use axum::extract::State as AxumState;
use axum::response::Response as AxumResponse;
use axum::routing::get;
use pretty_assertions::assert_eq;
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

#[test]
fn api_key_create_frame_is_byte_exact() {
    let original = " {\"type\":\"response.create\", \"model\":\"test\"} ".to_string();
    let mut sequence = 0;
    let got = prepare_client_text(
        original.clone(),
        &HeaderMap::new(),
        "acct_test",
        "req_test",
        false,
        &mut sequence,
    )
    .expect("valid frame");
    assert_eq!(got, original);
    assert_eq!(sequence, 0);
}

#[test]
fn subscription_create_normalization_preserves_websocket_fields() {
    let original = serde_json::json!({
        "type": "response.create",
        "model": "gpt-5.6-sol",
        "input": "hello",
        "tools": [],
        "generate": false,
        "previous_response_id": "resp_previous",
        "client_metadata": {"custom": "kept"}
    })
    .to_string();
    let mut sequence = 0;
    let got = prepare_client_text(
        original,
        &HeaderMap::new(),
        "acct_test",
        "req_test",
        true,
        &mut sequence,
    )
    .expect("valid frame");
    let value: Value = serde_json::from_str(&got).expect("normalized JSON");

    assert_eq!(value["type"], "response.create");
    assert_eq!(value["generate"], false);
    assert_eq!(value["previous_response_id"], "resp_previous");
    assert_eq!(value["client_metadata"]["custom"], "kept");
    assert_eq!(value["input"][0]["type"], "additional_tools");
    assert_eq!(value["store"], false);
    assert_eq!(sequence, 1);
}

#[test]
fn valid_non_create_application_event_is_byte_exact() {
    let original = "{\"type\":\"response.append_input_item\",\"item\":{}}".to_string();
    let mut sequence = 0;
    let got = prepare_client_text(
        original.clone(),
        &HeaderMap::new(),
        "acct_test",
        "req_test",
        true,
        &mut sequence,
    )
    .expect("valid control frame");
    assert_eq!(got, original);
    assert_eq!(sequence, 0);
}

#[test]
fn malformed_or_untyped_application_frames_are_rejected() {
    for frame in ["not-json", "{}", r#"{"type":""}"#] {
        assert!(
            prepare_client_text(
                frame.to_string(),
                &HeaderMap::new(),
                "acct_test",
                "req_test",
                false,
                &mut 0,
            )
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
async fn api_key_route_relays_multiple_turns_and_filters_handshake_headers() {
    let capture = WebSocketCapture::default();
    let app = Router::new()
        .route("/responses", get(accepting_upstream))
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
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
            .send(UpstreamMessage::Text(frame.to_string()))
            .await
            .expect("send create frame");
        let event = socket
            .next()
            .await
            .expect("completion event")
            .expect("valid completion event");
        let UpstreamMessage::Text(event) = event else {
            panic!("expected text completion event");
        };
        let value: Value = serde_json::from_str(&event).expect("event JSON");
        assert_eq!(value["sequence"], index + 1);
    }
    let _ = socket.close(CloseCode::Normal, None).await;

    assert_eq!(capture.frames.lock().await.as_slice(), [first, second]);
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
    for forbidden in [
        "cookie",
        "x-forwarded-for",
        mini_sub2api_protocol_v1::ACCOUNT_REF_HEADER,
        "sec-websocket-extensions",
    ] {
        assert!(!headers.contains_key(forbidden), "header {forbidden}");
    }
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
    state.websocket_client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all("http://127.0.0.1:1").expect("bad test proxy"))
        .build()
        .expect("proxied client");
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
    let send_result = socket.send(UpstreamMessage::Text(oversized)).await;
    if send_result.is_ok() {
        let closed = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("oversized close timeout");
        assert!(!matches!(closed, Some(Ok(UpstreamMessage::Text(_)))));
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
                    "response": {"usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}
                })
                .to_string();
                if socket.send(InternalMessage::Text(event.into())).await.is_err() {
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

async fn api_key_state(base_url: &str) -> (AppState, String, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-websocket-api-key-test".to_string(),
            format!("{base_url}/responses"),
        )
        .await
        .expect("API key record");
    let state = app_state(vault);
    (state, metadata.account_ref, temp)
}

fn app_state(vault: Vault) -> AppState {
    AppState {
        vault,
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client"),
        direct_client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .expect("direct client"),
        websocket_client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .http1_only()
            .build()
            .expect("WebSocket client"),
        direct_websocket_client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .http1_only()
            .no_proxy()
            .build()
            .expect("direct WebSocket client"),
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
        .header(mini_sub2api_protocol_v1::REQUEST_ID_HEADER, "req_ws_test")
}

#[path = "responses_websocket_oauth_tests.rs"]
mod oauth_tests;
