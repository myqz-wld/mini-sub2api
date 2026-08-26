use crate::fingerprint::FingerprintMode;
use crate::fingerprint::FingerprintSnapshot;
use crate::fingerprint_projection::project_websocket_device;
use crate::request_normalizer::EmulationTransport;
use crate::request_normalizer::SubscriptionIdentity;
use crate::request_normalizer::prepare_emulated_request;
use crate::request_normalizer::prepare_projected_subscription_websocket_turn;
use crate::request_profile::UpstreamProfile;
use crate::request_pseudonym::RequestPseudonymizer;
use crate::responses_websocket::MAX_WEBSOCKET_MESSAGE_BYTES;
use crate::responses_websocket_inject;
use crate::responses_websocket_state::PublicCreateMode;
use crate::responses_websocket_state::ResponsesWebSocketState;
use bytes::Bytes;
use http::HeaderMap;
use serde_json::Value;

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_client_text(
    text: String,
    headers: &mut HeaderMap,
    account_namespace: Option<&str>,
    profile: UpstreamProfile,
    pseudonym_scope: &str,
    fingerprint: &FingerprintSnapshot,
    continuation: &mut ResponsesWebSocketState,
) -> Result<String, ()> {
    let value: Value = serde_json::from_str(&text).map_err(|_| ())?;
    let message_type = value
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        .filter(|message_type| !message_type.is_empty())
        .ok_or(())?;
    if message_type == "response.inject" {
        return responses_websocket_inject::prepare(
            text,
            value,
            account_namespace,
            profile,
            pseudonym_scope,
            MAX_WEBSOCKET_MESSAGE_BYTES,
        );
    }
    if message_type != "response.create" {
        return Ok(text);
    }
    if profile == UpstreamProfile::BareOpenAi {
        continuation.plan_public_create(&value);
        return Ok(text);
    }
    let (prepared, synthesized_item_ids) = {
        let subscription_identity =
            account_namespace.map(|account_namespace| SubscriptionIdentity {
                account_namespace,
                downstream_scope: pseudonym_scope,
            });
        let prepared = if let Some(identity) = subscription_identity {
            prepare_projected_subscription_websocket_turn(
                headers,
                Bytes::from(text),
                MAX_WEBSOCKET_MESSAGE_BYTES,
                identity,
            )?
        } else {
            prepare_emulated_request(
                profile,
                EmulationTransport::WebSocket,
                headers,
                Bytes::from(text),
                MAX_WEBSOCKET_MESSAGE_BYTES,
                None,
            )?
        };
        let synthesized_item_ids = prepared.synthesized_item_ids;
        *headers = prepared.headers;
        (
            String::from_utf8(prepared.body.to_vec()).map_err(|_| ())?,
            synthesized_item_ids,
        )
    };
    let prepared = if fingerprint.mode() == FingerprintMode::Device
        && let Some(account_namespace) = account_namespace
    {
        let installation_id = RequestPseudonymizer::converged_installation_id(account_namespace);
        project_websocket_device(
            prepared,
            fingerprint,
            &installation_id,
            MAX_WEBSOCKET_MESSAGE_BYTES,
        )
        .map_err(|_| ())
    } else {
        Ok(prepared)
    }?;
    let value = serde_json::from_str::<Value>(&prepared).map_err(|_| ())?;
    plan_public_text_with_synthesized_ids(
        continuation,
        &value,
        &synthesized_item_ids,
        MAX_WEBSOCKET_MESSAGE_BYTES,
    )
}

#[cfg(test)]
pub(crate) fn plan_public_text(
    continuation: &mut ResponsesWebSocketState,
    value: &Value,
    maximum: usize,
) -> Result<String, ()> {
    plan_public_text_with_synthesized_ids(continuation, value, &[], maximum)
}

pub(crate) fn plan_public_text_with_synthesized_ids(
    continuation: &mut ResponsesWebSocketState,
    value: &Value,
    synthesized_item_ids: &[String],
    maximum: usize,
) -> Result<String, ()> {
    let plan = continuation.plan_public_create_with_synthesized_ids(value, synthesized_item_ids);
    debug_assert_ne!(plan.mode, PublicCreateMode::Passthrough);
    let encoded = encode_frame_bounded(&plan.frame, maximum);
    if encoded.is_ok() || plan.mode != PublicCreateMode::Incremental {
        return encoded;
    }
    continuation.fail_public_create();
    let fallback =
        continuation.plan_public_create_with_synthesized_ids(value, synthesized_item_ids);
    debug_assert_eq!(fallback.mode, PublicCreateMode::Full);
    encode_frame_bounded(&fallback.frame, maximum)
}

pub(crate) fn encode_frame_bounded(value: &Value, maximum: usize) -> Result<String, ()> {
    let encoded = serde_json::to_string(value).map_err(|_| ())?;
    if encoded.len() > maximum {
        Err(())
    } else {
        Ok(encoded)
    }
}
