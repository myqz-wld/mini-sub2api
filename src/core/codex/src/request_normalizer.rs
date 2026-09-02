use crate::fingerprint::FingerprintMode;
use crate::request_compaction::PendingCompaction;
use crate::request_identity;
use crate::request_identity_evidence::RequestIdentityEvidence;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_profile::UpstreamProfile;
use crate::request_state_editor::RequiredWireReferenceUnavailable;
use crate::request_state_resolution::resolve_and_project;
use crate::request_state_store::RequestStateStore;
use bytes::Bytes;
use http::HeaderMap;
use serde_json::Value;

#[path = "request_emulation_overlay.rs"]
mod overlay;

pub(crate) use request_identity::CodexTransport as EmulationTransport;

#[derive(Debug)]
pub struct PreparedEmulatedRequest {
    pub headers: HeaderMap,
    pub body: Bytes,
    pub(crate) synthesized_item_ids: Vec<String>,
    pub(crate) resolved_identity: Option<ResolvedRequestIdentity>,
    pub(crate) pending_compaction: Option<PendingCompaction>,
}

#[derive(Clone, Copy)]
pub(crate) struct CodexStateContext<'a> {
    pub(crate) account_ref: &'a str,
    pub(crate) state_namespace: &'a str,
    pub(crate) downstream_scope: &'a str,
    pub(crate) fingerprint_mode: FingerprintMode,
    pub(crate) store: &'a RequestStateStore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatefulPrepareError {
    InvalidRequest,
    StateUnavailable,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid stateful request projection")]
struct InvalidStatefulProjection {
    #[source]
    source: anyhow::Error,
}

/// Applies the Codex 0.149.0 request overlay selected by `upstream_profile`.
///
/// The caller object is cloned in full before the supported request-field allowlist and targeted
/// normalization are applied. `BareOpenAi` is deliberately rejected: callers must retain its
/// separate opaque-body path rather than treating normalization failure as permission to fall back
/// to bare forwarding.
#[cfg(test)]
pub(crate) fn prepare_codex_overlay_for_test(
    upstream_profile: UpstreamProfile,
    transport: EmulationTransport,
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
) -> Result<PreparedEmulatedRequest, ()> {
    prepare_codex_overlay(upstream_profile, transport, headers, body, max_bytes)
}

pub(crate) async fn prepare_stateful_codex_request(
    upstream_profile: UpstreamProfile,
    transport: EmulationTransport,
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    context: CodexStateContext<'_>,
    headers_already_projected: bool,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    if !upstream_profile.uses_identity_state() {
        return Err(StatefulPrepareError::InvalidRequest);
    }
    if has_non_identity_encoding(headers) {
        return Err(StatefulPrepareError::InvalidRequest);
    }
    let caller =
        serde_json::from_slice::<Value>(&body).map_err(|_| StatefulPrepareError::InvalidRequest)?;
    let caller = caller
        .as_object()
        .ok_or(StatefulPrepareError::InvalidRequest)?;
    validate_serialized_identity(caller, headers, headers_already_projected)?;
    let evidence =
        RequestIdentityEvidence::extract(caller, headers, transport, headers_already_projected);
    let pending = prepare_codex_overlay(upstream_profile, transport, headers, body, max_bytes)
        .map_err(|_| StatefulPrepareError::InvalidRequest)?;
    let mut value = serde_json::from_slice::<Value>(&pending.body)
        .map_err(|_| StatefulPrepareError::InvalidRequest)?;
    let mut object = value
        .as_object_mut()
        .ok_or(StatefulPrepareError::InvalidRequest)?
        .clone();
    let mut prepared_headers = pending.headers;
    let synthesized_item_ids = pending.synthesized_item_ids;
    let fingerprint_mode = context.fingerprint_mode;
    let max_bytes_for_edit = max_bytes;
    let prepared = context
        .store
        .edit(
            context.state_namespace,
            context.account_ref,
            context.downstream_scope,
            move |editor| {
                (|| {
                    let projection = resolve_and_project(
                        editor,
                        fingerprint_mode,
                        &evidence,
                        &mut prepared_headers,
                        &mut object,
                        &synthesized_item_ids,
                    )?;
                    let encoded = serde_json::to_vec(&Value::Object(object))?;
                    anyhow::ensure!(
                        encoded.len() <= max_bytes_for_edit,
                        "projected request is too large"
                    );
                    Ok(PreparedEmulatedRequest {
                        headers: prepared_headers,
                        body: Bytes::from(encoded),
                        synthesized_item_ids: projection.synthesized_item_ids,
                        resolved_identity: Some(projection.identity),
                        pending_compaction: projection.pending_compaction,
                    })
                })()
                .map_err(classify_projection_error)
            },
        )
        .await;
    prepared.map_err(|error| {
        if let Some(reference) = error.downcast_ref::<RequiredWireReferenceUnavailable>() {
            tracing::warn!(
                event = "required_request_reference_unavailable",
                domain = ?reference.domain,
            );
            StatefulPrepareError::StateUnavailable
        } else if let Some(invalid) = error.downcast_ref::<InvalidStatefulProjection>() {
            tracing::debug!(
                event = "invalid_stateful_request",
                error = %invalid.source,
            );
            StatefulPrepareError::InvalidRequest
        } else {
            tracing::warn!(event = "request_state_unavailable", error = %error);
            StatefulPrepareError::StateUnavailable
        }
    })
}

fn classify_projection_error(source: anyhow::Error) -> anyhow::Error {
    if source
        .downcast_ref::<RequiredWireReferenceUnavailable>()
        .is_some()
    {
        source
    } else {
        InvalidStatefulProjection { source }.into()
    }
}

fn prepare_codex_overlay(
    upstream_profile: UpstreamProfile,
    transport: EmulationTransport,
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
) -> Result<PreparedEmulatedRequest, ()> {
    if has_non_identity_encoding(headers) {
        return Err(());
    }

    let caller = serde_json::from_slice::<Value>(&body).map_err(|_| ())?;
    let mut value = Value::Object(caller.as_object().ok_or(())?.clone());
    let object = value
        .as_object_mut()
        .expect("caller object was cloned above");
    let mut prepared_headers = headers.clone();

    if !upstream_profile.emulates_codex() {
        return Err(());
    }

    let synthesized_item_ids =
        overlay::apply(object, &mut prepared_headers, transport, upstream_profile)?;
    let encoded = serde_json::to_vec(&value).map_err(|_| ())?;
    if encoded.len() > max_bytes {
        return Err(());
    }
    Ok(PreparedEmulatedRequest {
        headers: prepared_headers,
        body: Bytes::from(encoded),
        synthesized_item_ids,
        resolved_identity: None,
        pending_compaction: None,
    })
}

fn has_non_identity_encoding(headers: &HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let value = value.trim();
            !value.is_empty() && !value.eq_ignore_ascii_case("identity")
        })
}

fn validate_serialized_identity(
    object: &serde_json::Map<String, Value>,
    headers: &HeaderMap,
    headers_already_projected: bool,
) -> Result<(), StatefulPrepareError> {
    if let Some(raw) = object
        .get("client_metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get(crate::lifecycle_carriers::TURN_METADATA_HEADER))
    {
        let raw = raw.as_str().ok_or(StatefulPrepareError::InvalidRequest)?;
        if serde_json::from_str::<Value>(raw)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .is_none()
        {
            return Err(StatefulPrepareError::InvalidRequest);
        }
    }
    if !headers_already_projected
        && let Some(raw) = headers.get(crate::lifecycle_carriers::TURN_METADATA_HEADER)
    {
        let raw = raw
            .to_str()
            .map_err(|_| StatefulPrepareError::InvalidRequest)?;
        if serde_json::from_str::<Value>(raw)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .is_none()
        {
            return Err(StatefulPrepareError::InvalidRequest);
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) struct CodexStateTestHarness {
    _temp: tempfile::TempDir,
    store: RequestStateStore,
}

#[cfg(test)]
impl CodexStateTestHarness {
    pub(crate) fn new() -> Self {
        let temp = tempfile::tempdir().expect("stateful test directory");
        let accounts = temp.path().join("accounts");
        std::fs::create_dir(&accounts).expect("stateful test accounts directory");
        Self {
            _temp: temp,
            store: RequestStateStore::new(accounts),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn prepare(
        &self,
        profile: UpstreamProfile,
        transport: EmulationTransport,
        headers: &HeaderMap,
        body: Bytes,
        max_bytes: usize,
        account_ref: &str,
        state_namespace: &str,
        downstream_scope: &str,
    ) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
        prepare_stateful_codex_request(
            profile,
            transport,
            headers,
            body,
            max_bytes,
            CodexStateContext {
                account_ref,
                state_namespace,
                downstream_scope,
                fingerprint_mode: FingerprintMode::Device,
                store: &self.store,
            },
            false,
        )
        .await
    }
}

#[cfg(test)]
fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_string)
}

#[cfg(test)]
#[path = "request_normalizer_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "request_normalizer_message_tests.rs"]
mod message_tests;

#[cfg(test)]
#[path = "request_normalizer_defaults_tests.rs"]
mod defaults_tests;

#[cfg(test)]
#[path = "request_normalizer_native_ws_tests.rs"]
mod native_ws_tests;

#[cfg(test)]
#[path = "request_emulation_overlay_tests.rs"]
mod emulation_overlay_tests;

#[cfg(test)]
#[path = "request_emulation_protocol_tests.rs"]
mod emulation_protocol_tests;

#[cfg(test)]
#[path = "request_instructions_tests.rs"]
mod instructions_tests;

#[cfg(test)]
#[path = "request_normalizer_state_tests.rs"]
mod state_tests;

#[cfg(test)]
#[path = "request_normalizer_history_tests.rs"]
mod history_tests;
