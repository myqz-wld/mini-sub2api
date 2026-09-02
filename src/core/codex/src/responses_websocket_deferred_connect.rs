use super::DeferredCodexContext;
use crate::error::CoreFailure;
use crate::response_headers::provider_request_id;
use crate::response_headers::provider_request_id_control;
use crate::responses_websocket::send_handshake;
use crate::server::account_lock;
use crate::server::resolve_auth;
use crate::upstream_request::ResolvedAuth;
use crate::websocket_connector::WebSocketHandshake;
use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use http::HeaderMap;
use http::StatusCode;

pub(super) struct DeferredConnectFailure {
    pub(super) error: CoreFailure,
    pub(super) provider_request_id: Option<String>,
}

impl DeferredConnectFailure {
    fn without_response(error: CoreFailure) -> Self {
        Self {
            error,
            provider_request_id: None,
        }
    }
}

pub(super) async fn connect(
    context: &mut DeferredCodexContext,
    headers: &HeaderMap,
) -> Result<
    (
        crate::websocket_connector::WebSocketConnection,
        Option<http::HeaderValue>,
        Option<String>,
    ),
    DeferredConnectFailure,
> {
    let mut handshake = send_handshake(
        &context.resolved.transport,
        headers,
        &context.resolved.upstream_url,
        &context.resolved.auth,
        context.profile,
    )
    .await
    .map_err(DeferredConnectFailure::without_response)?;
    if handshake.status() == StatusCode::UNAUTHORIZED && context.profile.uses_oauth_refresh() {
        let initial_provider_request_id = provider_request_id(handshake.headers());
        let failed_access_token = match &context.resolved.auth {
            ResolvedAuth::CodexOAuth { token, .. } => token.clone(),
            ResolvedAuth::OpenAiApiKey { .. } => {
                return Err(DeferredConnectFailure {
                    error: CoreFailure::Internal,
                    provider_request_id: initial_provider_request_id,
                });
            }
        };
        let lock = account_lock(&context.state, &context.account_ref).await;
        let _guard = lock.lock().await;
        let retry = resolve_auth(
            &context.state,
            &context.account_ref,
            Some(&failed_access_token),
        )
        .await
        .map_err(|error| DeferredConnectFailure {
            error,
            provider_request_id: initial_provider_request_id.clone(),
        })?;
        drop(_guard);
        handshake = send_handshake(
            &retry.transport,
            headers,
            &retry.upstream_url,
            &retry.auth,
            context.profile,
        )
        .await
        .map_err(|error| DeferredConnectFailure {
            error,
            provider_request_id: initial_provider_request_id,
        })?;
        if handshake.status() == StatusCode::UNAUTHORIZED {
            return Err(DeferredConnectFailure {
                error: CoreFailure::UpstreamAuthFailed,
                provider_request_id: provider_request_id(handshake.headers()),
            });
        }
        if handshake.status() == StatusCode::SWITCHING_PROTOCOLS {
            context.resolved.upstream_url = retry.upstream_url;
            context.resolved.auth = retry.auth;
            context.resolved.transport = retry.transport;
            context.resolved.state_namespace = retry.state_namespace;
        }
    }
    if handshake.status() != StatusCode::SWITCHING_PROTOCOLS {
        return Err(DeferredConnectFailure {
            error: CoreFailure::UpstreamHandshakeRejected,
            provider_request_id: provider_request_id(handshake.headers()),
        });
    }
    let turn_state = handshake.headers().get("x-codex-turn-state").cloned();
    let raw_provider_request_id = provider_request_id(handshake.headers());
    match handshake {
        WebSocketHandshake::Connected { socket, .. } => {
            Ok((*socket, turn_state, raw_provider_request_id))
        }
        WebSocketHandshake::Rejected(response) => Err(DeferredConnectFailure {
            error: CoreFailure::Internal,
            provider_request_id: provider_request_id(response.headers()),
        }),
    }
}

pub(super) async fn send_provider_request_id_control(
    internal: &mut WebSocket,
    provider_request_id: Option<&str>,
) -> bool {
    let Some(provider_request_id) = provider_request_id else {
        return true;
    };
    let Ok(control) = provider_request_id_control(provider_request_id) else {
        return false;
    };
    internal.send(Message::Text(control.into())).await.is_ok()
}
