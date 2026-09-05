use super::*;
use crate::test_support::test_jwt;
use crate::vault::CredentialMaterial;
use axum::Json;
use axum::routing::post;
use pretty_assertions::assert_eq;
use reqwest_websocket::CloseCode as DownstreamCloseCode;
use reqwest_websocket::Message as DownstreamMessage;
use std::fs;
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Clone)]
struct OAuthWebSocketState {
    old_access: String,
    new_access: String,
    new_id: String,
    handshake_calls: Arc<AtomicUsize>,
    refresh_calls: Arc<AtomicUsize>,
    headers: Arc<Mutex<Option<HeaderMap>>>,
    frames: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone, Copy)]
enum HoldingProviderEvent {
    None,
    Immediate,
    AfterRelease,
}

#[derive(Clone)]
struct HoldingOAuthWebSocketState {
    event: HoldingProviderEvent,
    handshake_calls: Arc<AtomicUsize>,
    frames: Arc<Mutex<Vec<String>>>,
    create_seen: Arc<Notify>,
    release_event: Arc<Notify>,
}

struct HoldingOAuthFixture {
    _temp: tempfile::TempDir,
    _upstream: crate::test_support::LoopbackServer,
    state: HoldingOAuthWebSocketState,
    vault: Vault,
    account_ref: String,
    account_id: String,
}

#[path = "responses_websocket_oauth_handshake_tests.rs"]
mod handshake_tests;

#[path = "responses_websocket_oauth_failure_tests.rs"]
mod failure_tests;

async fn holding_oauth_fixture(
    account_id: &str,
    event: HoldingProviderEvent,
) -> HoldingOAuthFixture {
    let state = HoldingOAuthWebSocketState {
        event,
        handshake_calls: Arc::new(AtomicUsize::new(0)),
        frames: Arc::new(Mutex::new(Vec::new())),
        create_seen: Arc::new(Notify::new()),
        release_event: Arc::new(Notify::new()),
    };
    let app = Router::new()
        .route("/responses", get(holding_oauth_upstream))
        .with_state(state.clone());
    let upstream = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: test_jwt(Some(account_id), 7200),
                access_token: test_jwt(None, 7200),
                refresh_token: format!("refresh-{account_id}"),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                issuer: upstream.base_url.clone(),
                client_id: format!("client-{account_id}"),
            },
            format!("{}/responses", upstream.base_url),
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    HoldingOAuthFixture {
        _temp: temp,
        _upstream: upstream,
        state,
        vault,
        account_ref: metadata.account_ref,
        account_id: account_id.to_string(),
    }
}

fn stateful_create(turn: &str) -> String {
    serde_json::json!({
        "type":"response.create",
        "model":"gpt-5.4",
        "generate":true,
        "input":[{"type":"message","id":format!("msg_{turn}"),"role":"user","content":turn}],
        "client_metadata":{"session_id":"state-outage-session","turn_id":turn}
    })
    .to_string()
}

fn stateful_inject(response_id: &str) -> String {
    serde_json::json!({
        "type":"response.inject",
        "response_id":response_id,
        "input":[{
            "type":"function_call_output",
            "id":"fco_state_outage",
            "call_id":"call_state_outage",
            "output":"ok"
        }]
    })
    .to_string()
}

fn failure_metadata(message: DownstreamMessage) -> mini_sub2api_protocol_v1::FailureMetadata {
    let DownstreamMessage::Close { code, reason } = message else {
        panic!("expected failure close, got {message:?}");
    };
    assert_eq!(
        u16::from(code),
        mini_sub2api_protocol_v1::FAILURE_CLOSE_CODE
    );
    serde_json::from_str(&reason).expect("failure metadata")
}

async fn holding_oauth_upstream(
    AxumState(state): AxumState<HoldingOAuthWebSocketState>,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    state.handshake_calls.fetch_add(1, Ordering::SeqCst);
    let capture = state.clone();
    upgrade
        .on_upgrade(move |mut socket| async move {
            let Some(Ok(InternalMessage::Text(create))) = socket.next().await else {
                return;
            };
            capture.frames.lock().await.push(create.to_string());
            capture.create_seen.notify_one();
            match capture.event {
                HoldingProviderEvent::None => {}
                HoldingProviderEvent::Immediate => {}
                HoldingProviderEvent::AfterRelease => capture.release_event.notified().await,
            }
            if !matches!(capture.event, HoldingProviderEvent::None) {
                let event = serde_json::json!({
                    "type":"response.created",
                    "response":{"id":"resp_holding"}
                });
                if socket
                    .send(InternalMessage::Text(event.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
                capture.frames.lock().await.push(frame.to_string());
            }
        })
        .into_response()
}

#[path = "responses_websocket_oauth_reference_tests.rs"]
mod reference_tests;

async fn oauth_upstream(
    AxumState(state): AxumState<OAuthWebSocketState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    state.handshake_calls.fetch_add(1, Ordering::SeqCst);
    let authorization = header_text(&headers, http::header::AUTHORIZATION.as_str());
    if authorization.as_deref() == Some(format!("Bearer {}", state.old_access).as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": {"code": "expired"}})),
        )
            .into_response();
    }
    *state.headers.lock().await = Some(headers);
    let capture = state.clone();
    let mut response = upgrade
        .on_upgrade(move |mut socket| async move {
            for index in 0..2 {
                let Some(Ok(InternalMessage::Text(frame))) = socket.next().await else {
                    return;
                };
                capture.frames.lock().await.push(frame.to_string());
                let response_id = if index == 0 {
                    "resp_first"
                } else {
                    "resp_second"
                };
                let event = serde_json::json!({
                    "type":"response.completed",
                    "response":{
                        "id":response_id,
                        "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
                    }
                });
                let _ = socket
                    .send(InternalMessage::Text(event.to_string().into()))
                    .await;
            }
        })
        .into_response();
    response.headers_mut().insert(
        "x-codex-turn-state",
        HeaderValue::from_static("provider-turn-state"),
    );
    response
}

async fn oauth_upstream_unbounded(
    AxumState(state): AxumState<OAuthWebSocketState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    state.handshake_calls.fetch_add(1, Ordering::SeqCst);
    *state.headers.lock().await = Some(headers);
    let capture = state.clone();
    upgrade
        .on_upgrade(move |mut socket| async move {
            let mut index = 0_u64;
            while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
                capture.frames.lock().await.push(frame.to_string());
                let event = serde_json::json!({
                    "type":"response.completed",
                    "response":{
                        "id":format!("resp_state_{index}"),
                        "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
                    }
                });
                index = index.saturating_add(1);
                if socket
                    .send(InternalMessage::Text(event.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        })
        .into_response()
}
