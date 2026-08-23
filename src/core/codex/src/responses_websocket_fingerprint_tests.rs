use super::*;
use crate::fingerprint::FingerprintMode;
use crate::server::internal_router;
use crate::test_support::spawn_loopback;
use crate::vault::Vault;
use axum::Router;
use axum::extract::State as AxumState;
use axum::response::Response as AxumResponse;
use axum::routing::get;
use pretty_assertions::assert_eq;
use reqwest_websocket::CloseCode as DownstreamCloseCode;
use reqwest_websocket::Message as DownstreamMessage;
use reqwest_websocket::RequestBuilderExt;
use reqwest_websocket::WebSocket as DownstreamWebSocket;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const INTERNAL_TOKEN: &str = "internal-websocket-fingerprint-token-at-least-32-bytes";

#[derive(Clone, Default)]
struct FingerprintCapture {
    handshakes: Arc<Mutex<Vec<HeaderMap>>>,
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
async fn device_projects_handshake_and_create_frames_but_not_control_frames() {
    let capture = FingerprintCapture::default();
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", get(accepting_upstream))
            .with_state(capture.clone()),
    )
    .await;
    let (state, account_ref, _temp) =
        api_key_state(&upstream.base_url, FingerprintMode::Device).await;
    let expected_device = state
        .vault
        .fingerprint_snapshot(&account_ref)
        .await
        .expect("fingerprint")
        .installation_id()
        .to_string();
    let core = spawn_internal(state).await;
    let handshake = internal_handshake(&core.base_url, &account_ref)
        .header("x-codex-installation-id", "header-conflict")
        .header(
            "x-codex-turn-metadata",
            r#"{"installation_id":"header-turn-conflict","session_id":"header-session","future":1}"#,
        )
        .upgrade()
        .send()
        .await
        .expect("handshake");
    assert_eq!(handshake.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = handshake.into_websocket().await.expect("socket");

    let conflicting = serde_json::json!({
        "type": "response.create",
        "model": "first",
        "client_metadata": {
            "x-codex-installation-id": "body-conflict",
            "x-codex-turn-metadata": serde_json::json!({
                "installation_id": "body-turn-conflict",
                "session_id": "session-kept",
                "thread_id": "thread-kept",
                "turn_id": "turn-kept",
                "window_id": "window-kept",
                "future": {"kept": true}
            }).to_string()
        }
    })
    .to_string();
    send_and_receive(&mut socket, &conflicting).await;
    let prewarm = " {\"type\":\"response.create\", \"model\":\"prewarm\", \"generate\":false} ";
    send_and_receive(&mut socket, prewarm).await;
    let control = " {\"type\":\"response.append_input_item\", \"item\":{}} ";
    send_and_receive(&mut socket, control).await;
    let _ = socket.close(DownstreamCloseCode::Normal, None).await;

    let handshakes = capture.handshakes.lock().await;
    let upstream_headers = &handshakes[0];
    assert!(
        upstream_headers
            .get("x-codex-installation-id")
            .and_then(|value| value.to_str().ok())
            == Some(expected_device.as_str())
    );
    let header_turn: Value = serde_json::from_str(
        upstream_headers
            .get("x-codex-turn-metadata")
            .and_then(|value| value.to_str().ok())
            .expect("header turn metadata"),
    )
    .expect("header turn JSON");
    assert!(header_turn["installation_id"].as_str() == Some(expected_device.as_str()));
    assert_eq!(header_turn["session_id"], "header-session");
    assert_eq!(header_turn["future"], 1);
    drop(handshakes);

    let frames = capture.frames.lock().await;
    assert_eq!(frames.len(), 3);
    let create: Value = serde_json::from_str(&frames[0]).expect("create JSON");
    assert!(
        create["client_metadata"]["x-codex-installation-id"].as_str()
            == Some(expected_device.as_str())
    );
    let turn: Value = serde_json::from_str(
        create["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("turn metadata"),
    )
    .expect("turn JSON");
    assert!(turn["installation_id"].as_str() == Some(expected_device.as_str()));
    assert_eq!(turn["session_id"], "session-kept");
    assert_eq!(turn["thread_id"], "thread-kept");
    assert_eq!(turn["turn_id"], "turn-kept");
    assert_eq!(turn["window_id"], "window-kept");
    assert_eq!(turn["future"]["kept"], true);
    assert_eq!(frames[1], prewarm);
    assert_eq!(frames[2], control);
}

#[tokio::test]
async fn off_preserves_handshake_and_create_frame_identity() {
    let capture = FingerprintCapture::default();
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", get(accepting_upstream))
            .with_state(capture.clone()),
    )
    .await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url, FingerprintMode::Off).await;
    let core = spawn_internal(state).await;
    let handshake = internal_handshake(&core.base_url, &account_ref)
        .header("x-codex-installation-id", "caller-header-device")
        .upgrade()
        .send()
        .await
        .expect("handshake");
    let mut socket = handshake.into_websocket().await.expect("socket");
    let frame = r#" {"type":"response.create","client_metadata":{"x-codex-installation-id":"caller-body-device","x-codex-turn-metadata":"{\"installation_id\":\"caller-turn-device\"}"}} "#;
    send_and_receive(&mut socket, frame).await;

    assert_eq!(capture.frames.lock().await.as_slice(), [frame]);
    assert_eq!(
        capture.handshakes.lock().await[0]
            .get("x-codex-installation-id")
            .and_then(|value| value.to_str().ok()),
        Some("caller-header-device")
    );
}

#[tokio::test]
async fn stale_socket_forwards_zero_post_change_frames_and_reconnects() {
    let capture = FingerprintCapture::default();
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", get(accepting_upstream))
            .with_state(capture.clone()),
    )
    .await;
    let (state, account_ref, _temp) =
        api_key_state(&upstream.base_url, FingerprintMode::Device).await;
    let vault = state.vault.clone();
    let core = spawn_internal(state).await;
    let old_handshake = internal_handshake(&core.base_url, &account_ref)
        .upgrade()
        .send()
        .await
        .expect("old handshake");
    let mut old_socket = old_handshake.into_websocket().await.expect("old socket");

    vault
        .set_fingerprint_mode(&account_ref, FingerprintMode::Off)
        .await
        .expect("mode switch");
    let control = r#"{"type":"response.append_input_item","item":{"after":"switch"}}"#;
    send_and_receive(&mut old_socket, control).await;
    assert_eq!(capture.frames.lock().await.as_slice(), [control]);
    let frames_before_stale_create = capture.frames.lock().await.len();
    old_socket
        .send(DownstreamMessage::Text(
            r#"{"type":"response.create","model":"stale"}"#.to_string(),
        ))
        .await
        .expect("send stale create");
    let close = tokio::time::timeout(Duration::from_secs(2), old_socket.next())
        .await
        .expect("stale close timeout")
        .expect("stale close frame")
        .expect("valid stale close");
    let DownstreamMessage::Close { code, reason } = close else {
        panic!("expected stale close")
    };
    assert_eq!(code, DownstreamCloseCode::Restart);
    assert!(reason.is_empty());
    assert_eq!(
        capture.frames.lock().await.len(),
        frames_before_stale_create,
        "the stale create must not reach upstream"
    );

    let new_handshake = internal_handshake(&core.base_url, &account_ref)
        .header("x-codex-installation-id", "caller-after-switch")
        .upgrade()
        .send()
        .await
        .expect("new handshake");
    let mut new_socket = new_handshake.into_websocket().await.expect("new socket");
    let fresh = r#"{"type":"response.create","model":"fresh"}"#;
    send_and_receive(&mut new_socket, fresh).await;
    assert_eq!(capture.frames.lock().await.as_slice(), [control, fresh]);
    assert_eq!(
        capture.handshakes.lock().await[1]
            .get("x-codex-installation-id")
            .and_then(|value| value.to_str().ok()),
        Some("caller-after-switch")
    );
}

#[tokio::test]
async fn two_credentials_use_distinct_websocket_devices() {
    let capture = FingerprintCapture::default();
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", get(accepting_upstream))
            .with_state(capture.clone()),
    )
    .await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let first = create_api_key(&vault, &upstream.base_url, "first").await;
    let second = create_api_key(&vault, &upstream.base_url, "second").await;
    let first_device = vault
        .fingerprint_snapshot(&first)
        .await
        .expect("first fingerprint")
        .installation_id()
        .to_string();
    let second_device = vault
        .fingerprint_snapshot(&second)
        .await
        .expect("second fingerprint")
        .installation_id()
        .to_string();
    let core = spawn_internal(app_state(vault)).await;

    for account_ref in [&first, &second] {
        let handshake = internal_handshake(&core.base_url, account_ref)
            .upgrade()
            .send()
            .await
            .expect("handshake");
        drop(handshake.into_websocket().await.expect("socket"));
    }
    let handshakes = capture.handshakes.lock().await;
    let observed = handshakes
        .iter()
        .map(|headers| {
            headers
                .get("x-codex-installation-id")
                .and_then(|value| value.to_str().ok())
                .expect("device header")
        })
        .collect::<Vec<_>>();
    assert!(observed[0] == first_device);
    assert!(observed[1] == second_device);
    assert!(observed[0] != observed[1]);
}

#[tokio::test]
async fn malformed_device_handshake_metadata_never_reaches_upstream() {
    let capture = FingerprintCapture::default();
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", get(accepting_upstream))
            .with_state(capture.clone()),
    )
    .await;
    let (state, account_ref, _temp) =
        api_key_state(&upstream.base_url, FingerprintMode::Device).await;
    let core = spawn_internal(state).await;
    let response = internal_handshake(&core.base_url, &account_ref)
        .header("x-codex-turn-metadata", "not-json")
        .upgrade()
        .send()
        .await
        .expect("rejected handshake");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(capture.calls.load(Ordering::SeqCst), 0);
}

async fn accepting_upstream(
    AxumState(capture): AxumState<FingerprintCapture>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    capture.calls.fetch_add(1, Ordering::SeqCst);
    capture.handshakes.lock().await.push(headers);
    let relay_capture = capture.clone();
    upgrade
        .max_message_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .max_frame_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .on_upgrade(move |mut socket| async move {
            while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
                relay_capture.frames.lock().await.push(frame.to_string());
                let event = serde_json::json!({
                    "type": "response.completed",
                    "response": {"usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}
                })
                .to_string();
                if socket.send(InternalMessage::Text(event.into())).await.is_err() {
                    return;
                }
            }
        })
        .into_response()
}

async fn api_key_state(
    base_url: &str,
    mode: FingerprintMode,
) -> (AppState, String, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let account_ref = create_api_key(&vault, base_url, "test").await;
    if mode == FingerprintMode::Off {
        vault
            .set_fingerprint_mode(&account_ref, FingerprintMode::Off)
            .await
            .expect("off mode");
    }
    (app_state(vault), account_ref, temp)
}

async fn create_api_key(vault: &Vault, base_url: &str, label: &str) -> String {
    vault
        .create_api_key(
            format!("upstream-{label}-secret"),
            format!("{base_url}/responses"),
            FingerprintMode::Device,
        )
        .await
        .expect("API key")
        .account_ref
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
        .expect("bind internal server");
    let address = listener.local_addr().expect("internal address");
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            internal_router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve internal server");
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
        .header(mini_sub2api_protocol_v1::REQUEST_ID_HEADER, "req_ws_fp")
}

async fn send_and_receive(socket: &mut DownstreamWebSocket, frame: &str) {
    socket
        .send(DownstreamMessage::Text(frame.to_string()))
        .await
        .expect("send frame");
    let message = socket
        .next()
        .await
        .expect("completion")
        .expect("valid completion");
    assert!(matches!(message, DownstreamMessage::Text(_)));
}
