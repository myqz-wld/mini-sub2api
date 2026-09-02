use crate::fingerprint::FingerprintMode;
use crate::fingerprint::FingerprintSnapshot;
use crate::fingerprint_projection::project_websocket_device;
use crate::request_compaction::PendingCompaction;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_normalizer::CodexStateContext;
use crate::request_normalizer::EmulationTransport;
use crate::request_normalizer::prepare_stateful_codex_request;
use crate::request_profile::UpstreamProfile;
use crate::request_state_store::RequestStateStore;
use crate::responses_websocket::MAX_WEBSOCKET_MESSAGE_BYTES;
use crate::responses_websocket_inject;
use crate::responses_websocket_state::PublicCreateMode;
use crate::responses_websocket_state::ResponsesWebSocketState;
use bytes::Bytes;
use http::HeaderMap;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClientPrepareError {
    Protocol,
    StateUnavailable,
}

impl From<()> for ClientPrepareError {
    fn from((): ()) -> Self {
        Self::Protocol
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid WebSocket state projection")]
struct InvalidWebSocketStateProjection {
    #[source]
    source: anyhow::Error,
}

#[allow(clippy::too_many_arguments)]
pub(crate) struct PreparedClientText {
    pub(crate) text: String,
    pub(crate) create_value: Option<Value>,
    pub(crate) synthesized_item_ids: Vec<String>,
    pub(crate) pending_compaction: Option<PendingCompaction>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare_client_text(
    text: String,
    headers: &mut HeaderMap,
    account_ref: &str,
    state_namespace: Option<&str>,
    profile: UpstreamProfile,
    pseudonym_scope: &str,
    fingerprint: &FingerprintSnapshot,
    state_store: &RequestStateStore,
    identity_binding: &mut Option<ResolvedRequestIdentity>,
) -> Result<PreparedClientText, ClientPrepareError> {
    let text = if profile.uses_identity_state() {
        seed_socket_identity(text, identity_binding.as_ref())?
    } else {
        text
    };
    let value: Value = serde_json::from_str(&text).map_err(|_| ())?;
    let message_type = value
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        .filter(|message_type| !message_type.is_empty())
        .ok_or(())?;
    if message_type == "response.inject" {
        if profile.uses_identity_state() {
            let state_namespace = state_namespace.ok_or(())?;
            let filtered = responses_websocket_inject::prepare_without_identity(
                text,
                value,
                profile,
                MAX_WEBSOCKET_MESSAGE_BYTES,
            )?;
            let mut filtered = serde_json::from_str::<Value>(&filtered).map_err(|_| ())?;
            let filtered = state_store
                .edit(
                    state_namespace,
                    account_ref,
                    pseudonym_scope,
                    move |editor| {
                        (|| {
                            let object = filtered
                                .as_object_mut()
                                .ok_or_else(|| anyhow::anyhow!("inject frame is not an object"))?;
                            crate::request_wire_ids::translate_request_ids(
                                editor,
                                object,
                                &BTreeSet::new(),
                            )?;
                            Ok(filtered)
                        })()
                        .map_err(|source| InvalidWebSocketStateProjection { source }.into())
                    },
                )
                .await
                .map_err(classify_state_edit_error)?;
            let text = encode_frame_bounded(&filtered, MAX_WEBSOCKET_MESSAGE_BYTES)?;
            return Ok(PreparedClientText {
                text,
                create_value: None,
                synthesized_item_ids: Vec::new(),
                pending_compaction: None,
            });
        }
        let text =
            responses_websocket_inject::prepare(text, value, profile, MAX_WEBSOCKET_MESSAGE_BYTES)?;
        return Ok(PreparedClientText {
            text,
            create_value: None,
            synthesized_item_ids: Vec::new(),
            pending_compaction: None,
        });
    }
    if message_type != "response.create" {
        if profile.uses_identity_state() {
            let state_namespace = state_namespace.ok_or(())?;
            let original = value.clone();
            let translated = state_store
                .edit(
                    state_namespace,
                    account_ref,
                    pseudonym_scope,
                    move |editor| {
                        (|| {
                            let mut value = value;
                            let object = value
                                .as_object_mut()
                                .ok_or_else(|| anyhow::anyhow!("control frame is not an object"))?;
                            crate::request_wire_ids::translate_request_ids(
                                editor,
                                object,
                                &BTreeSet::new(),
                            )?;
                            Ok(value)
                        })()
                        .map_err(|source| InvalidWebSocketStateProjection { source }.into())
                    },
                )
                .await
                .map_err(classify_state_edit_error)?;
            let text = if translated == original {
                text
            } else {
                encode_frame_bounded(&translated, MAX_WEBSOCKET_MESSAGE_BYTES)?
            };
            return Ok(PreparedClientText {
                text,
                create_value: None,
                synthesized_item_ids: Vec::new(),
                pending_compaction: None,
            });
        }
        return Ok(PreparedClientText {
            text,
            create_value: None,
            synthesized_item_ids: Vec::new(),
            pending_compaction: None,
        });
    }
    if profile == UpstreamProfile::BareOpenAi {
        return Ok(PreparedClientText {
            text,
            create_value: Some(value),
            synthesized_item_ids: Vec::new(),
            pending_compaction: None,
        });
    }
    let (prepared, synthesized_item_ids, pending_compaction) = {
        let prepared = if profile.uses_identity_state() {
            let state_namespace = state_namespace.ok_or(())?;
            prepare_stateful_codex_request(
                profile,
                EmulationTransport::WebSocket,
                headers,
                Bytes::from(text),
                MAX_WEBSOCKET_MESSAGE_BYTES,
                CodexStateContext {
                    account_ref,
                    state_namespace,
                    downstream_scope: pseudonym_scope,
                    fingerprint_mode: fingerprint.mode(),
                    store: state_store,
                },
                true,
            )
            .await
            .map_err(|error| match error {
                crate::request_normalizer::StatefulPrepareError::InvalidRequest => {
                    ClientPrepareError::Protocol
                }
                crate::request_normalizer::StatefulPrepareError::StateUnavailable => {
                    ClientPrepareError::StateUnavailable
                }
            })?
        } else {
            return Err(ClientPrepareError::Protocol);
        };
        let synthesized_item_ids = prepared.synthesized_item_ids;
        let pending_compaction = prepared.pending_compaction;
        *headers = prepared.headers;
        if prepared.resolved_identity.is_some() {
            identity_binding.clone_from(&prepared.resolved_identity);
        }
        (
            String::from_utf8(prepared.body.to_vec()).map_err(|_| ())?,
            synthesized_item_ids,
            pending_compaction,
        )
    };
    let prepared = if fingerprint.mode() == FingerprintMode::Device && profile.uses_identity_state()
    {
        let installation_id = identity_binding
            .as_ref()
            .map(|identity| identity.installation_id.as_str())
            .ok_or(())?;
        project_websocket_device(
            prepared,
            fingerprint,
            installation_id,
            MAX_WEBSOCKET_MESSAGE_BYTES,
        )
        .map_err(|_| ())
    } else {
        Ok(prepared)
    }?;
    let value = serde_json::from_str::<Value>(&prepared).map_err(|_| ())?;
    Ok(PreparedClientText {
        text: prepared,
        create_value: Some(value),
        synthesized_item_ids,
        pending_compaction,
    })
}

fn classify_state_edit_error(error: anyhow::Error) -> ClientPrepareError {
    if error
        .downcast_ref::<InvalidWebSocketStateProjection>()
        .is_some()
    {
        ClientPrepareError::Protocol
    } else {
        ClientPrepareError::StateUnavailable
    }
}

fn seed_socket_identity(
    text: String,
    binding: Option<&ResolvedRequestIdentity>,
) -> Result<String, ()> {
    let Some(binding) = binding else {
        return Ok(text);
    };
    let mut value = serde_json::from_str::<Value>(&text).map_err(|_| ())?;
    let object = value.as_object_mut().ok_or(())?;
    if object.get("type").and_then(Value::as_str) != Some("response.create") {
        return Ok(text);
    }
    let has_user_input = object
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("role").and_then(Value::as_str) == Some("user"))
        });
    let metadata = object
        .entry("client_metadata".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !metadata.is_object() {
        *metadata = Value::Object(serde_json::Map::new());
    }
    let metadata = metadata.as_object_mut().ok_or(())?;
    metadata
        .entry("session_id".to_string())
        .or_insert_with(|| Value::String(binding.session_id.clone()));
    metadata
        .entry("thread_id".to_string())
        .or_insert_with(|| Value::String(binding.thread_id.clone()));
    if let Some(parent_thread_id) = binding.parent_thread_id.as_ref() {
        metadata
            .entry("parent_thread_id".to_string())
            .or_insert_with(|| Value::String(parent_thread_id.clone()));
    }
    if let Some(forked_from_thread_id) = binding.forked_from_thread_id.as_ref() {
        metadata
            .entry("forked_from_thread_id".to_string())
            .or_insert_with(|| Value::String(forked_from_thread_id.clone()));
    }
    if !has_user_input {
        if let Some(turn_id) = binding.turn_id.as_ref() {
            metadata
                .entry("turn_id".to_string())
                .or_insert_with(|| Value::String(turn_id.clone()));
        }
        if let Some(root_turn_id) = binding.root_turn_id.as_ref() {
            metadata
                .entry("root_turn_id".to_string())
                .or_insert_with(|| Value::String(root_turn_id.clone()));
        }
    }
    serde_json::to_string(&value).map_err(|_| ())
}

#[cfg(test)]
pub(crate) fn plan_public_text(
    continuation: &mut ResponsesWebSocketState,
    value: &Value,
    maximum: usize,
) -> Result<String, ()> {
    plan_public_text_with_synthesized_ids(continuation, value, &[], maximum)
}

#[cfg(test)]
pub(crate) fn plan_public_text_with_synthesized_ids(
    continuation: &mut ResponsesWebSocketState,
    value: &Value,
    synthesized_item_ids: &[String],
    maximum: usize,
) -> Result<String, ()> {
    plan_public_text_with_state(continuation, value, synthesized_item_ids, None, maximum)
}

pub(crate) fn plan_public_text_with_state(
    continuation: &mut ResponsesWebSocketState,
    value: &Value,
    synthesized_item_ids: &[String],
    pending_compaction: Option<PendingCompaction>,
    maximum: usize,
) -> Result<String, ()> {
    let retry_compaction = pending_compaction.clone();
    let plan =
        continuation.plan_public_create_with_state(value, synthesized_item_ids, pending_compaction);
    debug_assert_ne!(plan.mode, PublicCreateMode::Passthrough);
    let encoded = encode_frame_bounded(&plan.frame, maximum);
    if encoded.is_ok() || plan.mode != PublicCreateMode::Incremental {
        return encoded;
    }
    continuation.fail_public_create();
    let fallback =
        continuation.plan_public_create_with_state(value, synthesized_item_ids, retry_compaction);
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
