use crate::error::CoreFailure;
use crate::fingerprint::FingerprintMode;
use crate::fingerprint::FingerprintSnapshot;
use crate::fingerprint_projection::project_device_headers;
use crate::fingerprint_projection::project_websocket_device;
use crate::request_normalizer::prepare_subscription_request;
use crate::server::AppState;
use crate::server::account_lock;
use crate::server::header_text;
use crate::server::resolve_auth;
use crate::server::validate_internal_request;
use crate::transport_registry::CredentialTransportContext;
use crate::upstream_request::ResolvedAuth;
use crate::upstream_request::build_websocket;
use crate::vault::Vault;
use crate::websocket_connector::WebSocketConnection as UpstreamWebSocket;
use crate::websocket_connector::WebSocketHandshake;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::extract::State;
use axum::extract::ws::CloseFrame as InternalCloseFrame;
use axum::extract::ws::Message as InternalMessage;
use axum::extract::ws::WebSocket;
use axum::extract::ws::WebSocketUpgrade;
use axum::http::HeaderMap;
use axum::http::HeaderName;
use axum::http::HeaderValue;
use axum::http::Response;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use bytes::Bytes;
use futures_util::SinkExt;
use futures_util::StreamExt;
use mini_sub2api_protocol_v1::CORE_TTFB_HEADER;
use mini_sub2api_protocol_v1::REQUEST_ID_HEADER;
use serde_json::Value;
use std::net::SocketAddr;
use std::time::Instant;
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;
use tokio_tungstenite::tungstenite::protocol::CloseFrame as UpstreamCloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as UpstreamCloseCode;

pub(crate) const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_HANDSHAKE_REJECTION_BYTES: usize = 64 * 1024;

const SAFE_UPGRADE_RESPONSE_HEADERS: &[&str] = &[
    "openai-model",
    "x-codex-turn-state",
    "x-models-etag",
    "x-reasoning-included",
    "x-request-id",
];

const SAFE_REJECTION_RESPONSE_HEADERS: &[&str] = &["content-type", "retry-after", "x-request-id"];

pub(crate) async fn responses_socket(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response<Body> {
    let request_id = header_text(&headers, REQUEST_ID_HEADER).unwrap_or_default();
    match responses_socket_inner(peer, state, headers, upgrade).await {
        Ok(response) => response,
        Err(error) => error.into_response(request_id),
    }
}

async fn responses_socket_inner(
    peer: SocketAddr,
    state: AppState,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response<Body>, CoreFailure> {
    let identity = validate_internal_request(peer, &state, &headers)?;
    let account_lock = account_lock(&state, &identity.account_ref).await;
    let _guard = account_lock.lock().await;
    let resolved = resolve_auth(&state, &identity.account_ref, None).await?;
    drop(_guard);
    let fingerprint = resolved.fingerprint.clone();
    let mut upstream_headers = headers;
    if fingerprint.mode() == FingerprintMode::Device {
        project_device_headers(&mut upstream_headers, &fingerprint)
            .map_err(|_| CoreFailure::InvalidRequest)?;
    }

    let started = Instant::now();
    let mut handshake = send_handshake(
        &resolved.transport,
        &upstream_headers,
        &resolved.upstream_url,
        &resolved.auth,
    )
    .await?;
    let mut final_auth = resolved.auth;
    if handshake.status() == StatusCode::UNAUTHORIZED
        && matches!(final_auth, ResolvedAuth::CodexOAuth { .. })
    {
        let failed_access_token = match &final_auth {
            ResolvedAuth::CodexOAuth { token, .. } => token.clone(),
            ResolvedAuth::OpenAiApiKey { .. } => return Err(CoreFailure::Internal),
        };
        let _guard = account_lock.lock().await;
        let retry = resolve_auth(&state, &identity.account_ref, Some(&failed_access_token)).await?;
        drop(_guard);
        handshake = send_handshake(
            &retry.transport,
            &upstream_headers,
            &retry.upstream_url,
            &retry.auth,
        )
        .await?;
        final_auth = retry.auth;
        if handshake.status() == StatusCode::UNAUTHORIZED {
            return Err(CoreFailure::UpstreamAuthFailed);
        }
    }

    if handshake.status() != StatusCode::SWITCHING_PROTOCOLS {
        return Ok(rejection_response(handshake).await);
    }

    let response_headers = filtered_headers(handshake.headers(), SAFE_UPGRADE_RESPONSE_HEADERS);
    let upstream = match handshake {
        WebSocketHandshake::Connected { socket, .. } => *socket,
        WebSocketHandshake::Rejected(_) => return Err(CoreFailure::Internal),
    };
    let normalize_subscription = matches!(final_auth, ResolvedAuth::CodexOAuth { .. });
    let account_ref = identity.account_ref;
    let request_id = identity.request_id;
    let relay_context = RelayContext {
        headers: upstream_headers,
        account_ref,
        request_id,
        normalize_subscription,
        vault: state.vault.clone(),
        fingerprint,
    };
    let mut response = upgrade
        .max_message_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .max_frame_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .on_upgrade(move |internal| async move {
            relay(internal, upstream, relay_context).await;
        })
        .into_response();
    copy_headers(response.headers_mut(), &response_headers);
    if let Ok(value) = HeaderValue::from_str(&started.elapsed().as_millis().to_string()) {
        response.headers_mut().insert(CORE_TTFB_HEADER, value);
    }
    Ok(response)
}

async fn send_handshake(
    transport: &CredentialTransportContext,
    headers: &HeaderMap,
    upstream_url: &str,
    auth: &ResolvedAuth,
) -> Result<WebSocketHandshake, CoreFailure> {
    let (request, config) =
        build_websocket(headers, upstream_url, auth, MAX_WEBSOCKET_MESSAGE_BYTES)?;
    transport
        .websocket_connector_for_url(upstream_url)
        .connect(request, config)
        .await
        .map_err(|_| CoreFailure::UpstreamConnectFailed)
}

async fn rejection_response(handshake: WebSocketHandshake) -> Response<Body> {
    let WebSocketHandshake::Rejected(upstream) = handshake else {
        return Response::new(Body::empty());
    };
    let status = upstream.status();
    let headers = filtered_headers(upstream.headers(), SAFE_REJECTION_RESPONSE_HEADERS);
    let preserve_body = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let value = value.to_ascii_lowercase();
            value.starts_with("application/json") || value.starts_with("text/")
        });
    let body = if preserve_body {
        upstream
            .into_body()
            .filter(|body| body.len() <= MAX_HANDSHAKE_REJECTION_BYTES)
            .map(Bytes::from)
            .unwrap_or_default()
    } else {
        Bytes::new()
    };
    let mut response = Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()));
    copy_headers(response.headers_mut(), &headers);
    response
}

struct RelayContext {
    headers: HeaderMap,
    account_ref: String,
    request_id: String,
    normalize_subscription: bool,
    vault: Vault,
    fingerprint: FingerprintSnapshot,
}

async fn relay(internal: WebSocket, upstream: UpstreamWebSocket, context: RelayContext) {
    let RelayContext {
        headers,
        account_ref,
        request_id,
        normalize_subscription,
        vault,
        fingerprint,
    } = context;
    let (mut internal_write, mut internal_read) = internal.split();
    let (mut upstream_write, mut upstream_read) = upstream.split();
    let exit = {
        let client_to_upstream = async {
            let mut create_sequence = 0_u64;
            while let Some(message) = internal_read.next().await {
                let outbound = match message {
                    Ok(InternalMessage::Text(text)) => {
                        let text = text.to_string();
                        let is_create = match is_response_create(&text) {
                            Ok(is_create) => is_create,
                            Err(()) => {
                                let close = upstream_close(UpstreamCloseCode::Protocol);
                                let _ = upstream_write.send(close).await;
                                return RelayExit::Complete;
                            }
                        };
                        if is_create
                            && !fingerprint_is_current(&vault, &account_ref, &fingerprint).await
                        {
                            return RelayExit::StaleFingerprint;
                        }
                        match prepare_client_text(
                            text,
                            &headers,
                            &account_ref,
                            &request_id,
                            normalize_subscription,
                            &fingerprint,
                            &mut create_sequence,
                        ) {
                            Ok(prepared) => UpstreamMessage::Text(prepared.into()),
                            Err(()) => upstream_close(UpstreamCloseCode::Protocol),
                        }
                    }
                    Ok(InternalMessage::Binary(_)) => {
                        upstream_close(UpstreamCloseCode::Unsupported)
                    }
                    Ok(InternalMessage::Ping(payload)) => UpstreamMessage::Ping(payload),
                    Ok(InternalMessage::Pong(payload)) => UpstreamMessage::Pong(payload),
                    Ok(InternalMessage::Close(frame)) => {
                        let (code, reason) = frame
                            .map(|frame| (allowed_close_code(frame.code), frame.reason.to_string()))
                            .unwrap_or((UpstreamCloseCode::Normal, String::new()));
                        UpstreamMessage::Close(Some(UpstreamCloseFrame {
                            code,
                            reason: reason.into(),
                        }))
                    }
                    Err(_) => upstream_close(UpstreamCloseCode::Away),
                };
                let terminal = matches!(outbound, UpstreamMessage::Close(_));
                if upstream_write.send(outbound).await.is_err() || terminal {
                    return RelayExit::Complete;
                }
            }
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::Away))
                .await;
            RelayExit::Complete
        };
        let upstream_to_client = async {
            while let Some(message) = upstream_read.next().await {
                let outbound = match message {
                    Ok(UpstreamMessage::Text(text)) => {
                        InternalMessage::Text(text.to_string().into())
                    }
                    Ok(UpstreamMessage::Binary(_)) => internal_close(1003),
                    Ok(UpstreamMessage::Ping(payload)) => InternalMessage::Ping(payload),
                    Ok(UpstreamMessage::Pong(payload)) => InternalMessage::Pong(payload),
                    Ok(UpstreamMessage::Close(frame)) => {
                        InternalMessage::Close(frame.map(|frame| InternalCloseFrame {
                            code: u16::from(frame.code),
                            reason: frame.reason.to_string().into(),
                        }))
                    }
                    Ok(UpstreamMessage::Frame(_)) => internal_close(1011),
                    Err(_) => internal_close(1011),
                };
                let terminal = matches!(outbound, InternalMessage::Close(_));
                if internal_write.send(outbound).await.is_err() || terminal {
                    return RelayExit::Complete;
                }
            }
            let _ = internal_write.send(internal_close(1011)).await;
            RelayExit::Complete
        };
        tokio::select! {
            biased;
            exit = client_to_upstream => exit,
            exit = upstream_to_client => exit,
        }
    };
    if exit == RelayExit::StaleFingerprint {
        let _ = internal_write.send(internal_close(1012)).await;
        let _ = upstream_write
            .send(upstream_close(UpstreamCloseCode::Restart))
            .await;
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RelayExit {
    Complete,
    StaleFingerprint,
}

async fn fingerprint_is_current(
    vault: &Vault,
    account_ref: &str,
    captured: &FingerprintSnapshot,
) -> bool {
    let Ok(current) = vault.fingerprint_snapshot(account_ref).await else {
        return false;
    };
    current.revision() == captured.revision()
        && current.mode() == captured.mode()
        && current.installation_id() == captured.installation_id()
}

fn is_response_create(text: &str) -> Result<bool, ()> {
    let value: Value = serde_json::from_str(text).map_err(|_| ())?;
    let message_type = value
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        .filter(|message_type| !message_type.is_empty())
        .ok_or(())?;
    Ok(message_type == "response.create")
}

fn prepare_client_text(
    text: String,
    headers: &HeaderMap,
    account_ref: &str,
    request_id: &str,
    normalize_subscription: bool,
    fingerprint: &FingerprintSnapshot,
    create_sequence: &mut u64,
) -> Result<String, ()> {
    let value: Value = serde_json::from_str(&text).map_err(|_| ())?;
    let message_type = value
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        .filter(|message_type| !message_type.is_empty())
        .ok_or(())?;
    if message_type != "response.create" {
        return Ok(text);
    }
    let prepared = if normalize_subscription {
        *create_sequence = create_sequence.saturating_add(1);
        let frame_request_id = format!("{request_id}-ws-{create_sequence}");
        let prepared = prepare_subscription_request(
            headers,
            Bytes::from(text),
            MAX_WEBSOCKET_MESSAGE_BYTES,
            account_ref,
            &frame_request_id,
        );
        String::from_utf8(prepared.body.to_vec()).map_err(|_| ())?
    } else {
        text
    };
    if fingerprint.mode() == FingerprintMode::Device {
        project_websocket_device(prepared, fingerprint, MAX_WEBSOCKET_MESSAGE_BYTES).map_err(|_| ())
    } else {
        Ok(prepared)
    }
}

fn internal_close(code: u16) -> InternalMessage {
    InternalMessage::Close(Some(InternalCloseFrame {
        code,
        reason: "".into(),
    }))
}

fn upstream_close(code: UpstreamCloseCode) -> UpstreamMessage {
    UpstreamMessage::Close(Some(UpstreamCloseFrame {
        code,
        reason: "".into(),
    }))
}

fn allowed_close_code(code: u16) -> UpstreamCloseCode {
    let code = UpstreamCloseCode::from(code);
    if code.is_allowed() {
        code
    } else {
        UpstreamCloseCode::Protocol
    }
}

fn filtered_headers(source: &HeaderMap, allowed: &[&'static str]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for name in allowed {
        let name = HeaderName::from_static(name);
        for value in source.get_all(&name) {
            headers.append(name.clone(), value.clone());
        }
    }
    headers
}

fn copy_headers(destination: &mut HeaderMap, source: &HeaderMap) {
    for (name, value) in source {
        destination.append(name.clone(), value.clone());
    }
}

#[cfg(test)]
#[path = "responses_websocket_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "responses_websocket_fingerprint_tests.rs"]
mod fingerprint_tests;
