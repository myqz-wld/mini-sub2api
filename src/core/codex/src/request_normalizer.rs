use crate::fingerprint::FingerprintMode;
#[cfg(test)]
use crate::legacy_test_pseudonym::RequestPseudonymizer;
use crate::request_identity;
use crate::request_identity_evidence::RequestIdentityEvidence;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::request_profile::UpstreamProfile;
use crate::request_state_resolution::resolve_and_project;
use crate::request_state_store::RequestStateStore;
use bytes::Bytes;
use http::HeaderMap;
use serde_json::Value;

#[path = "request_emulation_overlay.rs"]
mod overlay;

pub(crate) use request_identity::SubscriptionTransport as EmulationTransport;

#[derive(Debug)]
pub struct PreparedEmulatedRequest {
    pub headers: HeaderMap,
    pub body: Bytes,
    pub(crate) synthesized_item_ids: Vec<String>,
    pub(crate) resolved_identity: Option<ResolvedRequestIdentity>,
}

#[cfg(test)]
pub type PreparedSubscriptionRequest = PreparedEmulatedRequest;

#[derive(Clone, Copy)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct SubscriptionIdentity<'a> {
    pub(crate) account_namespace: &'a str,
    pub(crate) downstream_scope: &'a str,
}

#[derive(Clone, Copy)]
pub(crate) struct SubscriptionStateContext<'a> {
    pub(crate) account_ref: &'a str,
    pub(crate) account_namespace: &'a str,
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
pub(crate) fn prepare_emulated_request(
    upstream_profile: UpstreamProfile,
    transport: EmulationTransport,
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    subscription_identity: Option<SubscriptionIdentity<'_>>,
) -> Result<PreparedEmulatedRequest, ()> {
    prepare_emulated_request_inner(
        upstream_profile,
        transport,
        headers,
        body,
        max_bytes,
        subscription_identity,
        false,
        true,
    )
}

pub(crate) async fn prepare_stateful_subscription_request(
    transport: EmulationTransport,
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    context: SubscriptionStateContext<'_>,
    headers_already_projected: bool,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    if has_non_identity_encoding(headers) {
        return Err(StatefulPrepareError::InvalidRequest);
    }
    let caller =
        serde_json::from_slice::<Value>(&body).map_err(|_| StatefulPrepareError::InvalidRequest)?;
    let caller = caller
        .as_object()
        .ok_or(StatefulPrepareError::InvalidRequest)?;
    let evidence =
        RequestIdentityEvidence::extract(caller, headers, transport, headers_already_projected);
    let pending = prepare_emulated_request_inner(
        UpstreamProfile::CodexSubscription149,
        transport,
        headers,
        body,
        max_bytes,
        Some(SubscriptionIdentity {
            account_namespace: context.account_namespace,
            downstream_scope: context.downstream_scope,
        }),
        headers_already_projected,
        false,
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
    let fingerprint_mode = context.fingerprint_mode;
    let max_bytes_for_edit = max_bytes;
    let prepared = context
        .store
        .edit(
            context.account_namespace,
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
                    })
                })()
                .map_err(|source| InvalidStatefulProjection { source }.into())
            },
        )
        .await;
    prepared.map_err(|error| {
        if let Some(invalid) = error.downcast_ref::<InvalidStatefulProjection>() {
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

#[allow(clippy::too_many_arguments)]
fn prepare_emulated_request_inner(
    upstream_profile: UpstreamProfile,
    transport: EmulationTransport,
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    subscription_identity: Option<SubscriptionIdentity<'_>>,
    _headers_already_projected: bool,
    pseudonymize_subscription: bool,
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

    match (upstream_profile, subscription_identity) {
        (UpstreamProfile::BareOpenAi, _) => return Err(()),
        (UpstreamProfile::CodexOpenAi149, None) => {}
        (UpstreamProfile::CodexSubscription149, Some(_identity)) => {
            if pseudonymize_subscription {
                #[cfg(not(test))]
                return Err(());
                #[cfg(test)]
                {
                    let pseudonymizer = RequestPseudonymizer::new(
                        _identity.account_namespace,
                        _identity.downstream_scope,
                    );
                    if _headers_already_projected {
                        pseudonymizer.apply_body_only(object)?;
                    } else {
                        pseudonymizer.apply(&mut prepared_headers, object)?;
                    }
                }
            }
        }
        (UpstreamProfile::CodexOpenAi149, Some(_))
        | (UpstreamProfile::CodexSubscription149, None) => return Err(()),
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
    })
}

#[cfg(test)]
pub fn prepare_subscription_request(
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    account_namespace: &str,
    downstream_scope: &str,
    _request_id: &str,
) -> Result<PreparedSubscriptionRequest, ()> {
    prepare_emulated_request(
        UpstreamProfile::CodexSubscription149,
        EmulationTransport::Http,
        headers,
        body,
        max_bytes,
        Some(SubscriptionIdentity {
            account_namespace,
            downstream_scope,
        }),
    )
}

#[cfg(test)]
pub fn prepare_websocket_subscription_request(
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    account_namespace: &str,
    downstream_scope: &str,
    _request_id: &str,
) -> Result<PreparedSubscriptionRequest, ()> {
    prepare_emulated_request(
        UpstreamProfile::CodexSubscription149,
        EmulationTransport::WebSocket,
        headers,
        body,
        max_bytes,
        Some(SubscriptionIdentity {
            account_namespace,
            downstream_scope,
        }),
    )
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
