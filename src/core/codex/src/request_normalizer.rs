use crate::fingerprint::FingerprintMode;
use crate::request_compaction::PendingCompaction;
use crate::request_identity;
use crate::request_identity_evidence::RequestIdentityEvidence;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_profile::UpstreamProfile;
use crate::request_state_editor::RequiredWireReferenceUnavailable;
use crate::request_state_resolution::resolve_and_project;
use crate::request_state_store::RequestStateStore;
pub(crate) use crate::subscription_normalizer::prepare_stateful_codex_request;
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
    pub(crate) lite_prefixes: Vec<usize>,
    pub(crate) resolved_identity: Option<ResolvedRequestIdentity>,
    pub(crate) pending_compaction: Option<PendingCompaction>,
    pub(crate) operation: Option<crate::subscription_context::Operation>,
}

pub(crate) struct CodexStateContext<'a> {
    pub(crate) account_ref: &'a str,
    pub(crate) state_namespace: &'a str,
    pub(crate) downstream_scope: &'a str,
    pub(crate) fingerprint_mode: FingerprintMode,
    pub(crate) store: &'a RequestStateStore,
    pub(crate) binding: Option<&'a ResolvedRequestIdentity>,
    pub(crate) force_lite: bool,
    pub(crate) admission: Option<(
        crate::subscription_prepare::ContextPlan,
        crate::subscription_request::Format,
    )>,
    pub(crate) socket_id: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum StatefulPrepareError {
    #[error("invalid request")]
    InvalidRequest,
    #[error("request state unavailable")]
    StateUnavailable,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid stateful request projection")]
struct InvalidStatefulProjection {
    #[source]
    source: anyhow::Error,
}

/// Applies the Codex 0.153.4 request overlay selected by `upstream_profile`.
///
/// The caller object is cloned in full before the supported request-field allowlist and targeted
/// normalization are applied. `ApiKeyPassthrough` is deliberately rejected: callers must retain its
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
    prepare_codex_overlay(upstream_profile, transport, headers, body, max_bytes, false)
}

pub(crate) async fn prepare_identity_request(
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
    let native = crate::request_native_metadata::NativeMetadata::read(caller, headers)
        .map_err(|_| StatefulPrepareError::InvalidRequest)?;
    let mut evidence =
        RequestIdentityEvidence::extract(caller, headers, transport, headers_already_projected);
    evidence.window_number = native.window_number.or(evidence.window_number);
    let pending = prepare_codex_overlay(
        upstream_profile,
        transport,
        headers,
        body,
        max_bytes,
        context.force_lite,
    )
    .map_err(|_| StatefulPrepareError::InvalidRequest)?;
    let mut value = serde_json::from_slice::<Value>(&pending.body)
        .map_err(|_| StatefulPrepareError::InvalidRequest)?;
    let mut object = value
        .as_object_mut()
        .ok_or(StatefulPrepareError::InvalidRequest)?
        .clone();
    let mut prepared_headers = pending.headers;
    let synthesized_item_ids = pending.synthesized_item_ids;
    let lite_prefixes = pending.lite_prefixes;
    let fingerprint_mode = context.fingerprint_mode;
    let max_bytes_for_edit = max_bytes;
    let cache = context.store.contexts.clone();
    let admission = context.admission;
    let connection_id = context.socket_id.map(str::to_string);
    let binding_session = context.binding.map(|binding| binding.session_id.clone());
    let prepared = context
        .store
        .edit(
            context.state_namespace,
            context.account_ref,
            context.downstream_scope,
            move |editor| {
                (|| {
                    let mut projection = resolve_and_project(
                        editor,
                        fingerprint_mode,
                        &evidence,
                        &mut prepared_headers,
                        &mut object,
                        &synthesized_item_ids,
                        &lite_prefixes,
                    )?;
                    projection.identity.connection_id = connection_id;
                    if binding_session
                        .as_ref()
                        .is_some_and(|session| *session != projection.identity.session_id)
                    {
                        return Err(StatefulPrepareError::InvalidRequest.into());
                    }
                    native.project(
                        editor,
                        &mut object,
                        &mut prepared_headers,
                        &projection.identity,
                    )?;
                    let encoded = serde_json::to_vec(&Value::Object(object))?;
                    anyhow::ensure!(
                        encoded.len() <= max_bytes_for_edit,
                        "projected request is too large"
                    );
                    let operation = admission
                        .map(|(plan, format)| cache.admit(plan, &projection.identity, format))
                        .transpose()?;
                    Ok(PreparedEmulatedRequest {
                        headers: prepared_headers,
                        body: Bytes::from(encoded),
                        synthesized_item_ids: projection.synthesized_item_ids,
                        lite_prefixes: Vec::new(),
                        resolved_identity: Some(projection.identity),
                        pending_compaction: projection.pending_compaction,
                        operation,
                    })
                })()
                .map_err(classify_projection_error)
            },
        )
        .await;
    let prepared = prepared.map_err(|error| {
        if let Some(error) = error.downcast_ref::<StatefulPrepareError>() {
            return *error;
        }
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
    })?;
    if let Some(operation) = &prepared.operation {
        context.store.contexts.commit_admission(operation)?;
    }
    Ok(prepared)
}

fn classify_projection_error(source: anyhow::Error) -> anyhow::Error {
    if source.downcast_ref::<StatefulPrepareError>().is_some() {
        return source;
    }
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
    force_lite: bool,
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

    let (synthesized_item_ids, lite_prefixes) = overlay::apply(
        object,
        &mut prepared_headers,
        transport,
        upstream_profile,
        force_lite,
    )?;
    let encoded = serde_json::to_vec(&value).map_err(|_| ())?;
    if encoded.len() > max_bytes {
        return Err(());
    }
    Ok(PreparedEmulatedRequest {
        headers: prepared_headers,
        body: Bytes::from(encoded),
        synthesized_item_ids,
        lite_prefixes,
        resolved_identity: None,
        pending_compaction: None,
        operation: None,
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
        prepare_identity_request(
            profile,
            transport,
            headers,
            body,
            max_bytes,
            CodexStateContext {
                force_lite: false,
                admission: None,
                binding: None,
                socket_id: None,
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
