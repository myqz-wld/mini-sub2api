use crate::error::CoreFailure;
use crate::fingerprint::FingerprintMode;
use crate::fingerprint_projection::project_device_headers;
use crate::fingerprint_projection::project_websocket_device;
use crate::request_identity::apply_synthetic_prewarm;
use crate::request_normalizer::CodexStateContext;
use crate::request_normalizer::EmulationTransport;
use crate::request_normalizer::StatefulPrepareError;
use crate::request_normalizer::prepare_stateful_codex_request;
use crate::request_profile::CallerKind;
use crate::request_profile::UpstreamProfile;
use crate::responses_websocket::MAX_WEBSOCKET_MESSAGE_BYTES;
use crate::responses_websocket::RelayContext;
use crate::responses_websocket::fingerprint_is_current;
use crate::responses_websocket::relay;
#[path = "responses_websocket_deferred_connect.rs"]
mod connect_support;
use crate::responses_websocket_emulation::encode_frame_bounded;
use crate::responses_websocket_emulation::plan_public_text_with_state;
use crate::responses_websocket_prewarm::HIDDEN_SETUP_TIMEOUT;
use crate::responses_websocket_prewarm::HiddenSetupOutcome;
use crate::responses_websocket_prewarm::prewarm_mode;
use crate::responses_websocket_prewarm::run_hidden_setup;
use crate::responses_websocket_state::ResponsesWebSocketState;
use crate::server::AppState;
use crate::server::ResolvedCredential;
use crate::websocket_delivery::failure_before_websocket_delivery;
use crate::websocket_delivery::failure_close;
use crate::websocket_delivery::internal_close;
use crate::websocket_delivery::is_response_create;
use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use bytes::Bytes;
use connect_support::connect;
use connect_support::send_provider_request_id_control;
use futures_util::StreamExt;
use http::HeaderMap;
use std::collections::VecDeque;
use std::future::Future;
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;

const MAX_DEFERRED_PENDING_MESSAGES: usize = 1024;
const DEFERRED_PENDING_MESSAGE_OVERHEAD: usize = 64;

pub(crate) struct DeferredCodexContext {
    pub(crate) state: AppState,
    pub(crate) headers: HeaderMap,
    pub(crate) account_ref: String,
    pub(crate) state_namespace: String,
    pub(crate) pseudonym_scope: String,
    pub(crate) caller: CallerKind,
    pub(crate) profile: UpstreamProfile,
    pub(crate) resolved: ResolvedCredential,
}

pub(crate) async fn run(mut internal: WebSocket, mut context: DeferredCodexContext) {
    let first = match first_create(&mut internal).await {
        Ok(first) => first,
        Err(code) => {
            let _ = internal.send(internal_close(code)).await;
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
        let _ = internal.send(internal_close(1012)).await;
        return;
    }
    let prepared = match prepare_stateful_codex_request(
        context.profile,
        EmulationTransport::WebSocket,
        &context.headers,
        Bytes::from(first),
        MAX_WEBSOCKET_MESSAGE_BYTES,
        CodexStateContext {
            account_ref: &context.account_ref,
            state_namespace: &context.state_namespace,
            downstream_scope: &context.pseudonym_scope,
            fingerprint_mode: context.resolved.fingerprint.mode(),
            store: context.state.vault.request_state(),
        },
        false,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(StatefulPrepareError::InvalidRequest) => {
            let _ = internal.send(internal_close(1002)).await;
            return;
        }
        Err(StatefulPrepareError::StateUnavailable) => {
            let _ = internal
                .send(failure_close(CoreFailure::StateUnavailable.failure()))
                .await;
            return;
        }
    };
    let synthesized_item_ids = prepared.synthesized_item_ids;
    let pending_compaction = prepared.pending_compaction;
    let mut upstream_headers = prepared.headers;
    let Some(resolved_identity) = prepared.resolved_identity else {
        let _ = internal.send(internal_close(1011)).await;
        return;
    };
    if context.resolved.fingerprint.mode() == FingerprintMode::Device
        && project_device_headers(&mut upstream_headers, &resolved_identity.installation_id)
            .is_err()
    {
        let _ = internal.send(internal_close(1002)).await;
        return;
    }
    let Ok(text) = String::from_utf8(prepared.body.to_vec()) else {
        let _ = internal.send(internal_close(1002)).await;
        return;
    };
    let text = if context.resolved.fingerprint.mode() == FingerprintMode::Device {
        match project_websocket_device(
            text,
            &context.resolved.fingerprint,
            &resolved_identity.installation_id,
            MAX_WEBSOCKET_MESSAGE_BYTES,
        ) {
            Ok(text) => text,
            Err(_) => {
                let _ = internal.send(internal_close(1002)).await;
                return;
            }
        }
    } else {
        text
    };
    let mut continuation = ResponsesWebSocketState::new(context.caller, context.profile);
    let value = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => value,
        Err(_) => {
            let _ = internal.send(internal_close(1002)).await;
            return;
        }
    };
    let mut handshake_headers = upstream_headers.clone();
    let hidden = match continuation.plan_hidden_setup_with_synthesized_ids(
        &value,
        prewarm_mode(&value),
        &synthesized_item_ids,
    ) {
        Some(mut hidden) => {
            let prepared = hidden.frame.as_object_mut().is_some_and(|frame| {
                apply_synthetic_prewarm(frame, &mut handshake_headers).is_ok()
            });
            if prepared {
                Some(hidden)
            } else {
                continuation.fail_hidden_setup();
                handshake_headers.clone_from(&upstream_headers);
                None
            }
        }
        None => None,
    };
    let mut pending = VecDeque::new();
    let mut pending_cost = 0_usize;
    let Some(connected) = wait_deferred(
        &mut internal,
        &mut pending,
        &mut pending_cost,
        connect(&mut context, &handshake_headers),
    )
    .await
    else {
        return;
    };
    let (mut upstream, mut turn_state, provider_request_id) = match connected {
        Ok(connected) => connected,
        Err(failure) => {
            if !send_provider_request_id_control(
                &mut internal,
                failure.provider_request_id.as_deref(),
            )
            .await
            {
                return;
            }
            let metadata = failure_before_websocket_delivery(&failure.error);
            let _ = internal.send(failure_close(metadata)).await;
            return;
        }
    };
    if !send_provider_request_id_control(&mut internal, provider_request_id.as_deref()).await {
        return;
    }
    if !fingerprint_is_current(
        &context.state.vault,
        &context.account_ref,
        &context.resolved.fingerprint,
    )
    .await
    {
        let _ = internal.send(internal_close(1012)).await;
        return;
    }
    if let Some(hidden) = hidden {
        let outcome =
            if let Ok(hidden) = encode_frame_bounded(&hidden.frame, MAX_WEBSOCKET_MESSAGE_BYTES) {
                let Some(outcome) = wait_deferred(
                    &mut internal,
                    &mut pending,
                    &mut pending_cost,
                    run_hidden_setup(
                        &mut upstream,
                        &mut continuation,
                        hidden,
                        HIDDEN_SETUP_TIMEOUT,
                    ),
                )
                .await
                else {
                    return;
                };
                outcome
            } else {
                continuation.fail_hidden_setup();
                HiddenSetupOutcome::Failed
            };
        if outcome == HiddenSetupOutcome::Reconnect {
            continuation.reset_for_reconnect();
            let Some(reconnected) = wait_deferred(
                &mut internal,
                &mut pending,
                &mut pending_cost,
                connect(&mut context, &upstream_headers),
            )
            .await
            else {
                return;
            };
            match reconnected {
                Ok((replacement, replacement_turn_state, replacement_request_id)) => {
                    upstream = replacement;
                    turn_state = replacement_turn_state;
                    if !send_provider_request_id_control(
                        &mut internal,
                        replacement_request_id.as_deref(),
                    )
                    .await
                    {
                        return;
                    }
                }
                Err(failure) => {
                    if !send_provider_request_id_control(
                        &mut internal,
                        failure.provider_request_id.as_deref(),
                    )
                    .await
                    {
                        return;
                    }
                    let metadata = failure_before_websocket_delivery(&failure.error);
                    let _ = internal.send(failure_close(metadata)).await;
                    return;
                }
            }
            if !fingerprint_is_current(
                &context.state.vault,
                &context.account_ref,
                &context.resolved.fingerprint,
            )
            .await
            {
                let _ = internal.send(internal_close(1012)).await;
                return;
            }
        }
    }
    debug_assert!(!continuation.public_create_attempted());
    let text = match plan_public_text_with_state(
        &mut continuation,
        &value,
        &synthesized_item_ids,
        pending_compaction,
        MAX_WEBSOCKET_MESSAGE_BYTES,
    ) {
        Ok(text) => text,
        Err(_) => {
            let _ = internal.send(internal_close(1002)).await;
            return;
        }
    };
    let mut relay_headers = upstream_headers;
    if let Some(turn_state) = turn_state {
        relay_headers.insert("x-codex-turn-state", turn_state);
    }
    let relay_context = RelayContext {
        headers: relay_headers,
        account_ref: context.account_ref,
        state_namespace: Some(context.state_namespace),
        pseudonym_scope: context.pseudonym_scope,
        profile: context.profile,
        continuation,
        pending,
        vault: context.state.vault,
        fingerprint: context.resolved.fingerprint,
        identity: Some(resolved_identity),
    };
    relay(
        internal,
        upstream,
        relay_context,
        Some(UpstreamMessage::Text(text.into())),
    )
    .await;
}

async fn wait_deferred<T>(
    internal: &mut WebSocket,
    pending: &mut VecDeque<Message>,
    pending_cost: &mut usize,
    future: impl Future<Output = T>,
) -> Option<T> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            output = &mut future => return Some(output),
            message = internal.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        match is_response_create(&text) {
                            Ok(true) => {
                                let _ = internal.send(internal_close(1008)).await;
                                return None;
                            }
                            Ok(false) => {
                                let Some(message_cost) = text.len().checked_add(DEFERRED_PENDING_MESSAGE_OVERHEAD) else {
                                    let _ = internal.send(internal_close(1009)).await;
                                    return None;
                                };
                                let Some(next) = pending_cost.checked_add(message_cost) else {
                                    let _ = internal.send(internal_close(1009)).await;
                                    return None;
                                };
                                if pending.len() >= MAX_DEFERRED_PENDING_MESSAGES
                                    || next > MAX_WEBSOCKET_MESSAGE_BYTES
                                {
                                    let _ = internal.send(internal_close(1009)).await;
                                    return None;
                                }
                                *pending_cost = next;
                                pending.push_back(Message::Text(text));
                            }
                            Err(()) => {
                                let _ = internal.send(internal_close(1002)).await;
                                return None;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if internal.send(Message::Pong(payload)).await.is_err() {
                            return None;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Binary(_))) => {
                        let _ = internal.send(internal_close(1003)).await;
                        return None;
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
                }
            }
        }
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
