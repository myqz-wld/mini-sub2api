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
use crate::responses_websocket::RelayContext;
use crate::responses_websocket::fingerprint_is_current;
use crate::responses_websocket::relay;
#[path = "responses_websocket_deferred_connect.rs"]
mod connect_support;
use crate::responses_websocket_emulation::encode_frame_bounded;
use crate::responses_websocket_emulation::plan_public_text_with_observations;
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
use axum::extract::ws::WebSocket;
use bytes::Bytes;
use connect_support::connect;
pub(crate) use connect_support::probe;
use connect_support::send_provider_request_id_control;
use http::HeaderMap;
use std::collections::VecDeque;
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;

#[path = "responses_websocket_deferred_input.rs"]
mod input;
use input::{first_create, wait_deferred};

pub(crate) struct DeferredCodexContext {
    pub(crate) state: AppState,
    pub(crate) headers: HeaderMap,
    pub(crate) account_ref: String,
    pub(crate) state_namespace: String,
    pub(crate) pseudonym_scope: String,
    pub(crate) caller: CallerKind,
    pub(crate) profile: UpstreamProfile,
    pub(crate) resolved: ResolvedCredential,
    pub(crate) reasoning_included: bool,
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
    let socket_lease = match context.state.vault.request_state().contexts.open_socket() {
        Ok(lease) => lease,
        Err(_) => {
            let _ = internal.send(internal_close(1011)).await;
            return;
        }
    };
    let prepared = match prepare_stateful_codex_request(
        context.profile,
        EmulationTransport::WebSocket,
        &context.headers,
        Bytes::from(first),
        crate::inference_limits::get().request_bytes,
        CodexStateContext {
            force_lite: false,
            admission: None,
            binding: None,
            socket_id: Some(&socket_lease.id),
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
    let operation = prepared.operation;
    let native_client_metadata = prepared.native_client_metadata;
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
            crate::inference_limits::get().request_bytes,
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
    if crate::request_classifier::selected(&upstream_headers) {
        continuation.disable_automatic_reuse();
    }
    continuation.mark_rebuilt_reference(prepared.rebuilt_reference);
    let mut value = match serde_json::from_str::<serde_json::Value>(&text) {
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
    let (mut upstream, _handshake_turn_state, provider_request_id, mut upgrade_metadata) =
        match connected {
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
                if !connect_support::send_protocol_failure(&mut internal, &failure).await {
                    return;
                }
                let _ = internal.send(failure_close(metadata)).await;
                return;
            }
        };
    if !send_provider_request_id_control(&mut internal, provider_request_id.as_deref()).await {
        return;
    }
    if !upgrade_metadata.matches_reasoning(context.reasoning_included) {
        let _ = internal
            .send(failure_close(
                CoreFailure::UpstreamHandshakeRejected.failure(),
            ))
            .await;
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
        let outcome = if let Ok(hidden) =
            encode_frame_bounded(&hidden.frame, crate::inference_limits::get().request_bytes)
        {
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
                Ok((replacement, _replacement_turn_state, replacement_request_id, metadata)) => {
                    if !send_provider_request_id_control(
                        &mut internal,
                        replacement_request_id.as_deref(),
                    )
                    .await
                    {
                        return;
                    }
                    if !metadata.matches_reasoning(context.reasoning_included) {
                        let _ = internal
                            .send(failure_close(
                                CoreFailure::UpstreamHandshakeRejected.failure(),
                            ))
                            .await;
                        return;
                    }
                    upstream = replacement;
                    upgrade_metadata = metadata;
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
                    if !connect_support::send_protocol_failure(&mut internal, &failure).await {
                        return;
                    }
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
    // Native ModelClient learns routing state from metadata events, independently of
    // caller metadata completeness. Native and hidden prewarm event tokens remain eligible.
    let mut turn_state: Option<http::HeaderValue> = None;
    if let Some(operation) = &operation {
        for token in [
            turn_state.as_ref().and_then(|v| v.to_str().ok()),
            continuation.setup_turn_state(),
        ]
        .into_iter()
        .flatten()
        {
            if context
                .state
                .vault
                .request_state()
                .contexts
                .learn_turn(operation, token)
                .is_err()
            {
                let _ = internal.send(internal_close(1011)).await;
                return;
            }
        }
        turn_state = context
            .state
            .vault
            .request_state()
            .contexts
            .turn_token(operation)
            .and_then(|token| token.parse().ok());
    }
    // Setup completion can learn routing state after the first request was normalized. Apply
    // the accepted turn token to the actual first business frame before planning/size checks.
    if let Some(token) = turn_state.as_ref().and_then(|value| value.to_str().ok()) {
        value["client_metadata"]["x-codex-turn-state"] =
            serde_json::Value::String(token.to_string());
        if !native_client_metadata
            && let Some(metadata) = value
                .get_mut("client_metadata")
                .and_then(serde_json::Value::as_object_mut)
        {
            crate::request_identity::randomize_synthesized_client_metadata(metadata);
        }
    }
    debug_assert!(!continuation.public_create_attempted());
    let (mut text, frame) = match plan_public_text_with_observations(
        &mut continuation,
        &value,
        &synthesized_item_ids,
        pending_compaction,
        crate::inference_limits::get().request_bytes,
    ) {
        Ok(text) => text,
        Err(_) => {
            let _ = internal.send(internal_close(1002)).await;
            return;
        }
    };
    if let Some(frame) = frame {
        text = match crate::request_state_editor::tool_observations::finalize_frame(
            context.state.vault.request_state(),
            &context.state_namespace,
            &context.account_ref,
            &context.pseudonym_scope,
            &resolved_identity.thread_id,
            frame,
            crate::inference_limits::get().request_bytes,
        )
        .await
        {
            Ok(text) => text,
            Err(_) => {
                let _ = internal
                    .send(failure_close(CoreFailure::StateUnavailable.failure()))
                    .await;
                return;
            }
        };
    }
    if let (Some(operation), Some(token)) = (
        &operation,
        turn_state.as_ref().and_then(|v| v.to_str().ok()),
    ) && context
        .state
        .vault
        .request_state()
        .contexts
        .learn_turn(operation, token)
        .is_err()
    {
        let _ = internal.send(internal_close(1011)).await;
        return;
    }
    let relay_context = RelayContext {
        headers: context.headers,
        account_ref: context.account_ref,
        state_namespace: Some(context.state_namespace),
        pseudonym_scope: context.pseudonym_scope,
        profile: context.profile,
        continuation,
        pending,
        vault: context.state.vault,
        fingerprint: context.resolved.fingerprint,
        auth_binding: crate::responses_websocket::auth_binding(
            &context.resolved.auth,
            &context.resolved.upstream_url,
        ),
        identity: Some(resolved_identity),
        operation,
        server_model: upgrade_metadata.model,
    };
    relay(
        internal,
        upstream,
        relay_context,
        Some(UpstreamMessage::Text(text.into())),
    )
    .await;
}
