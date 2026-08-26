use crate::request_identity;
use crate::request_profile::UpstreamProfile;
use crate::request_pseudonym::RequestPseudonymizer;
use bytes::Bytes;
use http::HeaderMap;
use serde_json::Value;

#[path = "request_emulation_overlay.rs"]
mod overlay;

pub(crate) use request_identity::SubscriptionTransport as EmulationTransport;

pub struct PreparedEmulatedRequest {
    pub headers: HeaderMap,
    pub body: Bytes,
    pub(crate) synthesized_item_ids: Vec<String>,
}

#[cfg(test)]
pub type PreparedSubscriptionRequest = PreparedEmulatedRequest;

#[derive(Clone, Copy)]
pub(crate) struct SubscriptionIdentity<'a> {
    pub(crate) account_namespace: &'a str,
    pub(crate) downstream_scope: &'a str,
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
    )
}

pub(crate) fn prepare_projected_subscription_websocket_turn(
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    subscription_identity: SubscriptionIdentity<'_>,
) -> Result<PreparedEmulatedRequest, ()> {
    prepare_emulated_request_inner(
        UpstreamProfile::CodexSubscription149,
        EmulationTransport::WebSocket,
        headers,
        body,
        max_bytes,
        Some(subscription_identity),
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn prepare_emulated_request_inner(
    upstream_profile: UpstreamProfile,
    transport: EmulationTransport,
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    subscription_identity: Option<SubscriptionIdentity<'_>>,
    headers_already_projected: bool,
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
        (UpstreamProfile::CodexSubscription149, Some(identity)) => {
            let pseudonymizer =
                RequestPseudonymizer::new(identity.account_namespace, identity.downstream_scope);
            if headers_already_projected {
                pseudonymizer.apply_body_only(object)?;
            } else {
                pseudonymizer.apply(&mut prepared_headers, object)?;
            }
        }
        (UpstreamProfile::CodexOpenAi149, Some(_))
        | (UpstreamProfile::CodexSubscription149, None) => return Err(()),
    }

    let synthesized_item_ids =
        overlay::apply(object, &mut prepared_headers, transport, upstream_profile);
    let encoded = serde_json::to_vec(&value).map_err(|_| ())?;
    if encoded.len() > max_bytes {
        return Err(());
    }
    Ok(PreparedEmulatedRequest {
        headers: prepared_headers,
        body: Bytes::from(encoded),
        synthesized_item_ids,
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
