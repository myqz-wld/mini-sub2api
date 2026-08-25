use crate::error::CoreFailure;
use crate::fingerprint::FingerprintMode;
use crate::fingerprint_projection::project_device_headers;
use crate::fingerprint_projection::project_websocket_device;
use crate::request_normalizer::prepare_websocket_subscription_request;
use crate::request_pseudonym::RequestPseudonymizer;
use crate::responses_websocket::MAX_WEBSOCKET_MESSAGE_BYTES;
use crate::responses_websocket::RelayContext;
use crate::responses_websocket::fingerprint_is_current;
use crate::responses_websocket::is_response_create;
use crate::responses_websocket::relay;
use crate::responses_websocket::send_handshake;
use crate::server::AppState;
use crate::server::ResolvedCredential;
use crate::server::account_lock;
use crate::server::resolve_auth;
use crate::upstream_request::ResolvedAuth;
use crate::websocket_connector::WebSocketHandshake;
use axum::extract::ws::CloseFrame;
use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use bytes::Bytes;
use futures_util::StreamExt;
use http::HeaderMap;
use http::StatusCode;
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;

pub(crate) struct DeferredOAuthContext {
    pub(crate) state: AppState,
    pub(crate) headers: HeaderMap,
    pub(crate) account_ref: String,
    pub(crate) account_namespace: String,
    pub(crate) pseudonym_scope: String,
    pub(crate) request_id: String,
    pub(crate) resolved: ResolvedCredential,
}

pub(crate) async fn run(mut internal: WebSocket, context: DeferredOAuthContext) {
    let first = match first_create(&mut internal).await {
        Ok(first) => first,
        Err(code) => {
            let _ = internal.send(close(code)).await;
            return;
        }
    };
    if !fingerprint_is_current(
        &context.state.vault,
        &context.account_ref,
        &context.resolved.fingerprint,
    )
    .await
    {
        let _ = internal.send(close(1012)).await;
        return;
    }
    let frame_request_id = format!("{}-ws-1", context.request_id);
    let Ok(prepared) = prepare_websocket_subscription_request(
        &context.headers,
        Bytes::from(first),
        MAX_WEBSOCKET_MESSAGE_BYTES,
        &context.account_namespace,
        &context.pseudonym_scope,
        &frame_request_id,
    ) else {
        let _ = internal.send(close(1002)).await;
        return;
    };
    let mut upstream_headers = prepared.headers;
    if context.resolved.fingerprint.mode() == FingerprintMode::Device
        && project_device_headers(
            &mut upstream_headers,
            &RequestPseudonymizer::converged_installation_id(&context.account_namespace),
        )
        .is_err()
    {
        let _ = internal.send(close(1002)).await;
        return;
    }
    let Ok(text) = String::from_utf8(prepared.body.to_vec()) else {
        let _ = internal.send(close(1002)).await;
        return;
    };
    let text = if context.resolved.fingerprint.mode() == FingerprintMode::Device {
        match project_websocket_device(
            text,
            &context.resolved.fingerprint,
            &RequestPseudonymizer::converged_installation_id(&context.account_namespace),
            MAX_WEBSOCKET_MESSAGE_BYTES,
        ) {
            Ok(text) => text,
            Err(_) => {
                let _ = internal.send(close(1002)).await;
                return;
            }
        }
    } else {
        text
    };
    let (upstream, turn_state) = match connect(&context, &upstream_headers).await {
        Ok(connected) => connected,
        Err(error) => {
            let code = if matches!(error, CoreFailure::UpstreamAuthFailed) {
                1008
            } else {
                1011
            };
            let _ = internal.send(close(code)).await;
            return;
        }
    };
    let mut relay_headers = context.headers;
    if let Some(turn_state) = turn_state {
        relay_headers.insert("x-codex-turn-state", turn_state);
    }
    let relay_context = RelayContext {
        headers: relay_headers,
        account_ref: context.account_ref,
        account_namespace: Some(context.account_namespace),
        pseudonym_scope: context.pseudonym_scope,
        request_id: context.request_id,
        normalize_subscription: true,
        vault: context.state.vault,
        fingerprint: context.resolved.fingerprint,
    };
    relay(
        internal,
        upstream,
        relay_context,
        Some(UpstreamMessage::Text(text.into())),
        1,
    )
    .await;
}

async fn connect(
    context: &DeferredOAuthContext,
    headers: &HeaderMap,
) -> Result<
    (
        crate::websocket_connector::WebSocketConnection,
        Option<http::HeaderValue>,
    ),
    CoreFailure,
> {
    let mut handshake = send_handshake(
        &context.resolved.transport,
        headers,
        &context.resolved.upstream_url,
        &context.resolved.auth,
    )
    .await?;
    if handshake.status() == StatusCode::UNAUTHORIZED {
        let failed_access_token = match &context.resolved.auth {
            ResolvedAuth::CodexOAuth { token, .. } => token.clone(),
            ResolvedAuth::OpenAiApiKey { .. } => return Err(CoreFailure::Internal),
        };
        let lock = account_lock(&context.state, &context.account_ref).await;
        let _guard = lock.lock().await;
        let retry = resolve_auth(
            &context.state,
            &context.account_ref,
            Some(&failed_access_token),
        )
        .await?;
        drop(_guard);
        handshake =
            send_handshake(&retry.transport, headers, &retry.upstream_url, &retry.auth).await?;
        if handshake.status() == StatusCode::UNAUTHORIZED {
            return Err(CoreFailure::UpstreamAuthFailed);
        }
    }
    if handshake.status() != StatusCode::SWITCHING_PROTOCOLS {
        return Err(CoreFailure::UpstreamConnectFailed);
    }
    let turn_state = handshake.headers().get("x-codex-turn-state").cloned();
    match handshake {
        WebSocketHandshake::Connected { socket, .. } => Ok((*socket, turn_state)),
        WebSocketHandshake::Rejected(_) => Err(CoreFailure::Internal),
    }
}

async fn first_create(internal: &mut WebSocket) -> Result<String, u16> {
    while let Some(message) = internal.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let text = text.to_string();
                return match is_response_create(&text) {
                    Ok(true) => Ok(text),
                    _ => Err(1002),
                };
            }
            Ok(Message::Ping(payload)) => {
                internal
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|_| 1011_u16)?;
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Binary(_)) => return Err(1003),
            Ok(Message::Close(_)) | Err(_) => return Err(1001),
        }
    }
    Err(1001)
}

fn close(code: u16) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: "".into(),
    }))
}
