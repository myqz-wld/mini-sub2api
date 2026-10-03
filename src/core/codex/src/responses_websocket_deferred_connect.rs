use super::DeferredCodexContext;
use crate::error::CoreFailure;
use crate::response_headers::provider_request_id;
use crate::response_headers::provider_request_id_control;
use crate::responses_websocket::send_handshake;
use crate::responses_websocket_upgrade_metadata::UpgradeMetadata;
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
    retry_metadata: Option<http::HeaderValue>,
}

impl DeferredConnectFailure {
    fn without_response(error: CoreFailure) -> Self {
        Self {
            error,
            provider_request_id: None,
            retry_metadata: None,
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
        UpgradeMetadata,
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
    for step in 0..2 {
        if handshake.status() != StatusCode::UNAUTHORIZED || !context.profile.uses_oauth_refresh() {
            break;
        }
        let initial_provider_request_id = provider_request_id(handshake.headers());
        let failed_access_token = match &context.resolved.auth {
            ResolvedAuth::CodexOAuth { token, .. } => token.clone(),
            ResolvedAuth::OpenAiApiKey { .. } => {
                return Err(DeferredConnectFailure::without_response(
                    CoreFailure::Internal,
                ));
            }
        };
        let lock = account_lock(&context.state, &context.account_ref).await;
        let guard = lock.lock().await;
        let retry = if step == 0 {
            crate::server::reload_auth(&context.state, &context.account_ref).await
        } else {
            resolve_auth(
                &context.state,
                &context.account_ref,
                Some(&failed_access_token),
            )
            .await
        }
        .map_err(|error| DeferredConnectFailure {
            error,
            provider_request_id: initial_provider_request_id.clone(),
            retry_metadata: None,
        })?;
        drop(guard);
        crate::server::validate_recovery_owner(&context.resolved, &retry)
            .map_err(DeferredConnectFailure::without_response)?;
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
            retry_metadata: None,
        })?;
        context.resolved = retry;
    }
    if handshake.status() == StatusCode::UNAUTHORIZED && context.profile.uses_oauth_refresh() {
        return Err(DeferredConnectFailure {
            error: CoreFailure::UpstreamAuthFailed,
            provider_request_id: provider_request_id(handshake.headers()),
            retry_metadata: None,
        });
    }
    if handshake.status() != StatusCode::SWITCHING_PROTOCOLS {
        let retry_metadata = match &handshake {
            WebSocketHandshake::Rejected(response) => response
                .body()
                .as_deref()
                .and_then(crate::response_failure::retry_metadata),
            _ => None,
        }
        .or_else(|| handshake.headers().get("x-retry-metadata").cloned());
        let error = match &handshake {
            WebSocketHandshake::Rejected(response) => response
                .body()
                .as_deref()
                .and_then(|body| crate::response_failure::http_category(response.status(), body)),
            _ => None,
        }
        .unwrap_or(CoreFailure::UpstreamHandshakeRejected);
        return Err(DeferredConnectFailure {
            error,
            provider_request_id: provider_request_id(handshake.headers()),
            retry_metadata,
        });
    }
    let turn_state = handshake.headers().get("x-codex-turn-state").cloned();
    let raw_provider_request_id = provider_request_id(handshake.headers());
    let metadata = UpgradeMetadata::read(handshake.headers());
    match handshake {
        WebSocketHandshake::Connected { socket, .. } => {
            Ok((*socket, turn_state, raw_provider_request_id, metadata))
        }
        WebSocketHandshake::Rejected(response) => Err(DeferredConnectFailure {
            error: CoreFailure::Internal,
            provider_request_id: provider_request_id(response.headers()),
            retry_metadata: None,
        }),
    }
}

pub(super) async fn send_protocol_failure(
    internal: &mut WebSocket,
    failure: &DeferredConnectFailure,
) -> bool {
    let error = &failure.error;
    if !error.is_native_response() {
        return true;
    }
    let mut event = serde_json::json!({"type":"error","status":error.status().as_u16(),
        "error":{"code":error.code(),"message":error.public_message()}});
    if let Some(kind) = error.native_error_type() {
        event["error"]["type"] = kind.into();
    }
    if let Some(minutes) = error.limit_window_minutes() {
        event["error"]["limit_window_minutes"] = minutes.into();
    }
    if let Some(value) = failure
        .retry_metadata
        .as_ref()
        .and_then(|value| value.to_str().ok())
    {
        event["error"]["headers"] = serde_json::json!({"x-retry-metadata":value});
    }
    internal
        .send(Message::Text(event.to_string().into()))
        .await
        .is_ok()
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
