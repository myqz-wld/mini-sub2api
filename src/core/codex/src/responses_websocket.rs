use crate::error::CoreFailure;
#[cfg(test)]
use crate::fingerprint::FingerprintMode;
use crate::fingerprint::FingerprintSnapshot;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_profile::{CallerKind, UpstreamProfile};
use crate::response_translation::ResponseStateContext;
use crate::responses_websocket_deferred::DeferredOAuthContext;
use crate::responses_websocket_emulation::ClientPrepareError;
pub(crate) use crate::responses_websocket_emulation::prepare_client_text;
use crate::responses_websocket_http::{copy_headers, filtered_upgrade_headers, rejection_response};
use crate::responses_websocket_state::{EventDisposition, OperationPhase, ResponsesWebSocketState};
use crate::server::{AppState, account_lock, header_text, resolve_auth, validate_internal_request};
use crate::transport_registry::CredentialTransportContext;
use crate::upstream_request::{ResolvedAuth, build_websocket};
use crate::vault::Vault;
use crate::websocket_connector::{WebSocketConnection as UpstreamWebSocket, WebSocketHandshake};
use crate::websocket_delivery::{
    WebSocketDeliveryTracker, failure_close, internal_close, is_response_create,
    is_terminal_response_event,
};
use axum::body::Body;
use axum::extract::ws::{
    CloseFrame as InternalCloseFrame, Message as InternalMessage, WebSocket, WebSocketUpgrade,
};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use mini_sub2api_protocol_v1::CORE_TTFB_HEADER;
use mini_sub2api_protocol_v1::FailurePhase;
use mini_sub2api_protocol_v1::REQUEST_ID_HEADER;
use serde_json::Value;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::Instant;
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;
use tokio_tungstenite::tungstenite::protocol::CloseFrame as UpstreamCloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as UpstreamCloseCode;

pub(crate) const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

#[path = "responses_websocket_initial.rs"]
mod initial;

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
    let caller = CallerKind::from_headers(&headers);
    let account_lock = account_lock(&state, &identity.account_ref).await;
    let _guard = account_lock.lock().await;
    let resolved = resolve_auth(&state, &identity.account_ref, None).await?;
    drop(_guard);
    let profile = UpstreamProfile::select(caller, resolved.auth.credential_kind());
    if profile.uses_codex_subscription() {
        let account_namespace = match &resolved.auth {
            ResolvedAuth::CodexOAuth { account_id, .. } => account_id.clone(),
            ResolvedAuth::OpenAiApiKey { .. } => return Err(CoreFailure::Internal),
        };
        let context = DeferredOAuthContext {
            state,
            headers,
            account_ref: identity.account_ref,
            account_namespace,
            pseudonym_scope: identity.pseudonym_scope,
            caller,
            profile,
            resolved,
        };
        return Ok(upgrade
            .max_message_size(MAX_WEBSOCKET_MESSAGE_BYTES)
            .max_frame_size(MAX_WEBSOCKET_MESSAGE_BYTES)
            .on_upgrade(move |internal| async move {
                crate::responses_websocket_deferred::run(internal, context).await;
            })
            .into_response());
    }
    let fingerprint = resolved.fingerprint.clone();
    let upstream_headers = headers;

    let started = Instant::now();
    let handshake = send_handshake(
        &resolved.transport,
        &upstream_headers,
        &resolved.upstream_url,
        &resolved.auth,
        profile,
    )
    .await?;
    if handshake.status() != StatusCode::SWITCHING_PROTOCOLS {
        return Ok(rejection_response(handshake).await);
    }

    let response_headers = filtered_upgrade_headers(handshake.headers());
    let upstream = match handshake {
        WebSocketHandshake::Connected { socket, .. } => *socket,
        WebSocketHandshake::Rejected(_) => return Err(CoreFailure::Internal),
    };
    let account_ref = identity.account_ref;
    let relay_context = RelayContext {
        headers: upstream_headers,
        account_ref,
        account_namespace: None,
        pseudonym_scope: identity.pseudonym_scope,
        profile,
        continuation: ResponsesWebSocketState::new(caller, profile),
        pending: VecDeque::new(),
        vault: state.vault.clone(),
        fingerprint,
        identity: None,
    };
    let mut response = upgrade
        .max_message_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .max_frame_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .on_upgrade(move |internal| async move {
            relay(internal, upstream, relay_context, None).await;
        })
        .into_response();
    copy_headers(response.headers_mut(), &response_headers);
    if let Ok(value) = HeaderValue::from_str(&started.elapsed().as_millis().to_string()) {
        response.headers_mut().insert(CORE_TTFB_HEADER, value);
    }
    Ok(response)
}

pub(crate) async fn send_handshake(
    transport: &CredentialTransportContext,
    headers: &HeaderMap,
    upstream_url: &str,
    auth: &ResolvedAuth,
    profile: UpstreamProfile,
) -> Result<WebSocketHandshake, CoreFailure> {
    let (request, config) = build_websocket(
        headers,
        upstream_url,
        auth,
        profile,
        MAX_WEBSOCKET_MESSAGE_BYTES,
    )?;
    transport
        .websocket_connector_for_url(upstream_url)
        .connect(request, config)
        .await
        .map_err(|_| CoreFailure::UpstreamConnectFailed)
}

pub(crate) struct RelayContext {
    pub(crate) headers: HeaderMap,
    pub(crate) account_ref: String,
    pub(crate) account_namespace: Option<String>,
    pub(crate) pseudonym_scope: String,
    pub(crate) profile: UpstreamProfile,
    pub(crate) continuation: ResponsesWebSocketState,
    pub(crate) pending: VecDeque<InternalMessage>,
    pub(crate) vault: Vault,
    pub(crate) fingerprint: FingerprintSnapshot,
    pub(crate) identity: Option<ResolvedRequestIdentity>,
}

pub(crate) async fn relay(
    internal: WebSocket,
    upstream: UpstreamWebSocket,
    context: RelayContext,
    initial: Option<UpstreamMessage>,
) {
    let RelayContext {
        mut headers,
        account_ref,
        account_namespace,
        pseudonym_scope,
        profile,
        continuation,
        mut pending,
        vault,
        fingerprint,
        mut identity,
    } = context;
    let (mut internal_write, mut internal_read) = internal.split();
    let (mut upstream_write, mut upstream_read) = upstream.split();
    let delivery = WebSocketDeliveryTracker::default();
    let continuation = Arc::new(StdMutex::new(continuation));
    let response_state = account_namespace.as_deref().and_then(|namespace| {
        profile.uses_codex_subscription().then(|| {
            ResponseStateContext::new(
                &account_ref,
                namespace,
                &pseudonym_scope,
                vault.request_state(),
                identity.as_ref(),
            )
        })
    });
    let exit = {
        let client_continuation = Arc::clone(&continuation);
        let client_response_state = response_state.clone();
        let client_to_upstream = async {
            if let Some(initial) = initial {
                if !fingerprint_is_current(&vault, &account_ref, &fingerprint).await {
                    return RelayExit::StaleFingerprint;
                }
                if let Err(exit) = initial::send(
                    &mut internal_read,
                    &mut upstream_write,
                    initial,
                    &mut pending,
                    &client_continuation,
                    &delivery,
                )
                .await
                {
                    return exit;
                }
            }
            loop {
                let message = if let Some(message) = pending.pop_front() {
                    Ok(message)
                } else {
                    let Some(message) = internal_read.next().await else {
                        break;
                    };
                    message
                };
                let mut create_attempt = false;
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
                        if is_create && public_create_in_flight(&client_continuation) {
                            return RelayExit::Policy;
                        }
                        let prepared = prepare_client_text(
                            text,
                            &mut headers,
                            &account_ref,
                            if profile.uses_codex_subscription() {
                                account_namespace.as_deref()
                            } else {
                                None
                            },
                            profile,
                            &pseudonym_scope,
                            &fingerprint,
                            vault.request_state(),
                            &mut identity,
                        )
                        .await;
                        match prepared {
                            Err(ClientPrepareError::StateUnavailable) => {
                                return RelayExit::StateUnavailable(
                                    delivery.failure_for_phase(FailurePhase::Internal),
                                );
                            }
                            Err(ClientPrepareError::Protocol) => {
                                upstream_close(UpstreamCloseCode::Protocol)
                            }
                            Ok(prepared) => {
                                let planned = if let Some(value) = prepared.create_value.as_ref() {
                                    let mut continuation = continuation_guard(&client_continuation);
                                    if profile == UpstreamProfile::BareOpenAi {
                                        continuation.plan_public_create(value);
                                        Ok(prepared.text)
                                    } else {
                                        crate::responses_websocket_emulation::plan_public_text_with_synthesized_ids(
                                            &mut continuation,
                                            value,
                                            &prepared.synthesized_item_ids,
                                            MAX_WEBSOCKET_MESSAGE_BYTES,
                                        )
                                    }
                                } else {
                                    Ok(prepared.text)
                                };
                                match planned {
                                    Ok(prepared) => {
                                        if is_create
                                            && let Some(state) = client_response_state.as_ref()
                                            && state.update_identity(identity.as_ref()).is_err()
                                        {
                                            return RelayExit::StateUnavailable(
                                                delivery.failure_for_phase(FailurePhase::Internal),
                                            );
                                        }
                                        create_attempt = is_create;
                                        UpstreamMessage::Text(prepared.into())
                                    }
                                    Err(()) => upstream_close(UpstreamCloseCode::Protocol),
                                }
                            }
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
                if create_attempt {
                    if !continuation_guard(&client_continuation).mark_public_create_attempted() {
                        return RelayExit::Policy;
                    }
                    delivery.mark_attempted();
                }
                if upstream_write.send(outbound).await.is_err() {
                    if create_attempt {
                        continuation_guard(&client_continuation).fail_public_create();
                    }
                    return if terminal {
                        RelayExit::Complete
                    } else {
                        RelayExit::Failure(delivery.failure())
                    };
                }
                if terminal {
                    return RelayExit::Complete;
                }
            }
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::Away))
                .await;
            RelayExit::Complete
        };
        let server_continuation = Arc::clone(&continuation);
        let upstream_to_client = async {
            while let Some(message) = upstream_read.next().await {
                let (outbound, terminal_event) = match message {
                    Ok(UpstreamMessage::Text(text)) => {
                        let upstream_text = text.to_string();
                        let disposition = observe_server_text(&server_continuation, &upstream_text);
                        if disposition == EventDisposition::ConsumeHiddenSetup {
                            continue;
                        }
                        delivery.mark_response_observed();
                        let text = match response_state.as_ref() {
                            Some(state) => match state
                                .translate_text(upstream_text.clone(), MAX_WEBSOCKET_MESSAGE_BYTES)
                                .await
                            {
                                Ok(text) => text,
                                Err(_) => return RelayExit::Failure(delivery.failure()),
                            },
                            None => upstream_text.clone(),
                        };
                        let terminal = is_terminal_response_event(&upstream_text);
                        (InternalMessage::Text(text.into()), terminal)
                    }
                    Ok(UpstreamMessage::Binary(_)) | Ok(UpstreamMessage::Frame(_)) => {
                        return RelayExit::Failure(delivery.failure());
                    }
                    Ok(UpstreamMessage::Ping(payload)) => (InternalMessage::Ping(payload), false),
                    Ok(UpstreamMessage::Pong(payload)) => (InternalMessage::Pong(payload), false),
                    Ok(UpstreamMessage::Close(frame)) => {
                        let failure = delivery.failure();
                        if failure.delivery_state
                            != mini_sub2api_protocol_v1::DeliveryState::NotDelivered
                        {
                            return RelayExit::Failure(failure);
                        }
                        (
                            InternalMessage::Close(frame.map(|frame| InternalCloseFrame {
                                code: u16::from(frame.code),
                                reason: frame.reason.to_string().into(),
                            })),
                            false,
                        )
                    }
                    Err(_) => return RelayExit::Failure(delivery.failure()),
                };
                let terminal = matches!(outbound, InternalMessage::Close(_));
                if internal_write.send(outbound).await.is_err() {
                    return RelayExit::Complete;
                }
                if terminal_event {
                    delivery.mark_terminal();
                }
                if terminal {
                    return RelayExit::Complete;
                }
            }
            RelayExit::Failure(delivery.failure())
        };
        tokio::select! {
            biased;
            exit = client_to_upstream => exit,
            exit = upstream_to_client => exit,
        }
    };
    match exit {
        RelayExit::Complete => continuation_guard(&continuation).reset(),
        RelayExit::StaleFingerprint => {
            continuation_guard(&continuation).reset();
            let _ = internal_write.send(internal_close(1012)).await;
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::Restart))
                .await;
        }
        RelayExit::Failure(metadata) => {
            continuation_guard(&continuation).fail_public_create();
            let _ = internal_write.send(failure_close(metadata)).await;
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::Restart))
                .await;
        }
        RelayExit::StateUnavailable(metadata) => {
            continuation_guard(&continuation).reset();
            let _ = internal_write.send(failure_close(metadata)).await;
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::Restart))
                .await;
        }
        RelayExit::Policy => {
            continuation_guard(&continuation).reset();
            let _ = internal_write.send(internal_close(1008)).await;
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::Policy))
                .await;
        }
        RelayExit::TooLarge => {
            continuation_guard(&continuation).reset();
            let _ = internal_write.send(internal_close(1009)).await;
            let _ = upstream_write
                .send(upstream_close(UpstreamCloseCode::Size))
                .await;
        }
    }
}

#[derive(Clone, Copy)]
enum RelayExit {
    Complete,
    StaleFingerprint,
    Failure(mini_sub2api_protocol_v1::FailureMetadata),
    StateUnavailable(mini_sub2api_protocol_v1::FailureMetadata),
    Policy,
    TooLarge,
}

pub(crate) async fn fingerprint_is_current(
    vault: &Vault,
    account_ref: &str,
    captured: &FingerprintSnapshot,
) -> bool {
    let Ok(current) = vault.fingerprint_snapshot(account_ref).await else {
        return false;
    };
    current.revision() == captured.revision() && current.mode() == captured.mode()
}

fn public_create_in_flight(continuation: &StdMutex<ResponsesWebSocketState>) -> bool {
    matches!(
        continuation_guard(continuation).public_phase(),
        OperationPhase::Attempted | OperationPhase::ResponseObserved
    )
}

fn observe_server_text(
    continuation: &StdMutex<ResponsesWebSocketState>,
    text: &str,
) -> EventDisposition {
    let mut continuation = continuation_guard(continuation);
    match serde_json::from_str::<Value>(text) {
        Ok(event) => continuation.observe_server_event(&event),
        Err(_) if public_phase_in_flight(continuation.public_phase()) => {
            continuation.fail_public_create();
            EventDisposition::ForwardPublic
        }
        Err(_) => EventDisposition::Unassociated,
    }
}

fn public_phase_in_flight(phase: OperationPhase) -> bool {
    matches!(
        phase,
        OperationPhase::Attempted | OperationPhase::ResponseObserved
    )
}

fn continuation_guard(
    continuation: &StdMutex<ResponsesWebSocketState>,
) -> MutexGuard<'_, ResponsesWebSocketState> {
    continuation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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

#[cfg(test)]
#[path = "responses_websocket_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "responses_websocket_fingerprint_tests.rs"]
mod fingerprint_tests;
