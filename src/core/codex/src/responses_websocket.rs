use crate::error::CoreFailure;
#[cfg(test)]
use crate::fingerprint::FingerprintMode;
use crate::fingerprint::FingerprintSnapshot;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_profile::{CallerKind, UpstreamProfile};
use crate::response_translation::ResponseStateContext;
use crate::responses_websocket_deferred::DeferredCodexContext;
use crate::responses_websocket_emulation::ClientPrepareError;
pub(crate) use crate::responses_websocket_emulation::prepare_client_text;
use crate::responses_websocket_http::{copy_headers, filtered_upgrade_headers, rejection_response};
use crate::responses_websocket_state::{
    EventDisposition, ObservedServerEvent, OperationPhase, ResponsesWebSocketState,
};
use crate::server::{AppState, account_lock, header_text, resolve_auth, validate_internal_request};
use crate::vault::Vault;
use crate::websocket_connector::{WebSocketConnection as UpstreamWebSocket, WebSocketHandshake};
use crate::websocket_delivery::{
    WebSocketDeliveryTracker, failure_close, internal_close, is_response_create,
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
use tracing::Instrument;

#[path = "responses_websocket_inbound.rs"]
mod inbound;
#[path = "responses_websocket_initial.rs"]
mod initial;
#[path = "responses_websocket_relay_helpers.rs"]
mod relay_helpers;
use relay_context::RelayExit;
use relay_helpers::allowed_close_code;
pub(crate) use relay_helpers::auth_binding;
use relay_helpers::continuation_guard;
pub(crate) use relay_helpers::fingerprint_is_current;
use relay_helpers::observe_server_event;
use relay_helpers::public_create_in_flight;
use relay_helpers::upstream_close;

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
    let gateway_request_id =
        header_text(&headers, REQUEST_ID_HEADER).ok_or(CoreFailure::InvalidRequest)?;
    let caller = CallerKind::from_headers(&headers);
    let account_lock = account_lock(&state, &identity.account_ref).await;
    let _guard = account_lock.lock().await;
    let resolved = resolve_auth(&state, &identity.account_ref, None).await?;
    drop(_guard);
    let profile = UpstreamProfile::select(caller, resolved.auth.credential_kind());
    if profile.uses_identity_state() {
        let mut context = DeferredCodexContext {
            state,
            headers,
            account_ref: identity.account_ref,
            state_namespace: resolved.state_namespace.clone(),
            pseudonym_scope: identity.pseudonym_scope,
            caller,
            profile,
            resolved,
            reasoning_included: false,
        };
        let metadata = match crate::responses_websocket_deferred::probe(&mut context).await {
            Ok(metadata) => metadata,
            Err(failure) => return Ok(failure.into_probe_response(gateway_request_id)),
        };
        context.reasoning_included = metadata.reasoning.is_some();
        let mut response = upgrade
            .max_message_size(crate::inference_limits::get().request_bytes)
            .max_frame_size(crate::inference_limits::get().request_bytes)
            .on_upgrade(move |internal| async move {
                crate::responses_websocket_deferred::run(internal, context)
                    .instrument(tracing::info_span!(
                        "websocket_request",
                        request_id = gateway_request_id
                    ))
                    .await;
            })
            .into_response();
        if let Some(value) = metadata.reasoning {
            response.headers_mut().insert(
                crate::responses_websocket_upgrade_metadata::REASONING_HEADER,
                value,
            );
        }
        return Ok(response);
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
        return Ok(rejection_response(handshake, &gateway_request_id).await);
    }

    let response_headers = filtered_upgrade_headers(handshake.headers(), &gateway_request_id)
        .map_err(|_| CoreFailure::UpstreamResponseFailed)?;
    let upstream = match handshake {
        WebSocketHandshake::Connected { socket, .. } => *socket,
        WebSocketHandshake::Rejected(_) => return Err(CoreFailure::Internal),
    };
    let account_ref = identity.account_ref;
    let relay_context = RelayContext {
        headers: upstream_headers,
        account_ref,
        state_namespace: None,
        pseudonym_scope: identity.pseudonym_scope,
        profile,
        continuation: ResponsesWebSocketState::new(caller, profile),
        pending: VecDeque::new(),
        vault: state.vault.clone(),
        fingerprint,
        auth_binding: relay_helpers::auth_binding(&resolved.auth, &resolved.upstream_url),
        identity: None,
        operation: None,
        server_model: None,
    };
    let mut response = upgrade
        .max_message_size(crate::inference_limits::get().request_bytes)
        .max_frame_size(crate::inference_limits::get().request_bytes)
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

pub(crate) use crate::responses_websocket_http::send_handshake;

#[path = "responses_websocket_relay_context.rs"]
mod relay_context;
pub(crate) use relay_context::RelayContext;

#[path = "responses_websocket_relay.rs"]
mod relay;
pub(crate) use relay::relay;

#[cfg(test)]
#[path = "responses_websocket_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "responses_websocket_fingerprint_tests.rs"]
mod fingerprint_tests;
