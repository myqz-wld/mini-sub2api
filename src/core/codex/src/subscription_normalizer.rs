use crate::request_normalizer::{
    CodexStateContext, EmulationTransport, PreparedEmulatedRequest, StatefulPrepareError as Error,
    prepare_identity_request,
};
use crate::request_profile::UpstreamProfile;
use crate::subscription_context::ContextStore;
use crate::subscription_request::{Evidence, Format};
use bytes::Bytes;
use http::HeaderMap;
use serde_json::Value;

pub(crate) async fn prepare_stateful_codex_request(
    profile: UpstreamProfile,
    transport: EmulationTransport,
    headers: &HeaderMap,
    body: Bytes,
    max_bytes: usize,
    mut context: CodexStateContext<'_>,
    _headers_already_projected: bool,
) -> Result<PreparedEmulatedRequest, Error> {
    let later_headers;
    let headers = if transport == EmulationTransport::WebSocket && context.binding.is_some() {
        later_headers = crate::subscription_handshake::bound_headers(headers)
            .map_err(|_| Error::InvalidRequest)?;
        &later_headers
    } else {
        headers
    };
    if !profile.uses_identity_state() {
        return Err(Error::InvalidRequest);
    }
    let mut object = serde_json::from_slice::<serde_json::Map<String, Value>>(&body)
        .map_err(|_| Error::InvalidRequest)?;
    let identity_evidence = crate::request_identity_evidence::RequestIdentityEvidence::extract(
        &object, headers, transport, false,
    );
    crate::request_native_metadata::NativeMetadata::read(&object, headers)
        .map_err(|_| Error::InvalidRequest)?;
    let evidence = Evidence::read(&object, headers, transport)?;
    let store = &context.store.contexts;
    let plan = store.plan(
        ContextStore::scope_key(context.state_namespace, context.downstream_scope),
        &object,
        evidence,
        context.binding,
        context.socket_id,
    )?;
    let target_lite = plan.caller_format == Format::Lite
        || object
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| crate::request_defaults::model_profile(model).responses_lite);
    if target_lite
        && object
            .get("reasoning")
            .and_then(Value::as_object)
            .and_then(|r| r.get("context"))
            .is_some_and(|context| context.as_str() != Some("all_turns"))
    {
        return Err(Error::InvalidRequest);
    }
    let upstream_format = if target_lite {
        Format::Lite
    } else {
        Format::Responses
    };
    let explicit_delta = plan.evidence.previous.is_some();
    let mut full_send = transport == EmulationTransport::Http;
    if explicit_delta && transport == EmulationTransport::WebSocket {
        let base = plan.baseline.as_ref().expect("validated baseline");
        full_send = base.socket.as_deref() != context.socket_id || context.socket_id.is_none();
        if base.upstream_format != upstream_format {
            full_send = true;
        }
        if target_lite {
            // A changed ordinary base/tool prefix cannot be installed by pretending it is native Lite.
            if base.setup_hash != crate::subscription_index::setup_hash(&plan.settings) {
                full_send = true;
            }
        }
    }
    if explicit_delta && full_send {
        let full = plan.full_input(store)?;
        object.insert("input".into(), Value::Array(full));
        object.remove("previous_response_id");
        if let Some(settings) = plan.settings.as_object() {
            for (name, value) in settings {
                object.entry(name).or_insert_with(|| value.clone());
            }
        }
    } else {
        object.insert("input".into(), Value::Array(plan.evidence.input.clone()));
    }
    if explicit_delta && !full_send && target_lite {
        // The validated unchanged prefix is already held upstream. Ordinary callers retain their
        // own classification; this only changes the selected WS payload projection.
        object.remove("tools");
        object.remove("instructions");
    }
    if plan.caller_format == Format::Lite {
        // Identical mixed tool carriers are one deterministic, lossless repair.
        if let Some(prefix) = object
            .get("input")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            && prefix.get("type").and_then(Value::as_str) == Some("additional_tools")
        {
            object.remove("tools");
        }
    }
    let session = plan
        .session
        .clone()
        .or_else(|| plan.evidence.session.clone())
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let turn = plan
        .turn
        .clone()
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let metadata = object
        .entry("client_metadata")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or(Error::InvalidRequest)?;
    metadata.insert("session_id".into(), Value::String(session));
    if let Some(inherited) = plan
        .baseline
        .as_ref()
        .map(|base| &base.identity)
        .or(context.binding)
        && (!identity_evidence.explicit_thread_lineage
            || (identity_evidence.parent_thread.is_none() && identity_evidence.thread.is_none()))
        && inherited.thread_id != inherited.session_id
    {
        metadata.insert(
            "thread_id".into(),
            Value::String(inherited.thread_id.clone()),
        );
        metadata.insert(
            "parent_thread_id".into(),
            Value::String(
                inherited
                    .parent_thread_id
                    .clone()
                    .unwrap_or_else(|| inherited.session_id.clone()),
            ),
        );
    }
    if let Some(base) = &plan.baseline
        && identity_evidence.window_number.is_none()
    {
        metadata.insert(
            "x-codex-window-id".into(),
            Value::String(base.identity.window_id()),
        );
    }
    if identity_evidence.is_prewarm() {
        metadata.insert("turn_id".into(), Value::String(String::new()));
    } else if identity_evidence.is_memory() {
        metadata.remove("turn_id");
    } else {
        metadata.insert("turn_id".into(), Value::String(turn));
    }
    metadata.remove("x-codex-turn-state");
    let mut clean_headers = headers.clone();
    clean_headers.remove("x-codex-turn-state");
    // Use the selected session, but retain the other original first-request carriers. Bound WS
    // handshakes have already shed their stale turn/window/branch evidence above.
    clean_headers.remove("session-id");
    let assembly_limit = if transport == EmulationTransport::WebSocket {
        store.limits.session_bytes
    } else {
        max_bytes
    };
    let encoded = serde_json::to_vec(&object).map_err(|_| Error::InvalidRequest)?;
    if encoded.len() > assembly_limit {
        return Err(Error::InvalidRequest);
    }
    context.force_lite = target_lite;
    context.admission = Some((plan, upstream_format));
    let mut prepared = prepare_identity_request(
        profile,
        transport,
        &clean_headers,
        Bytes::from(encoded),
        assembly_limit,
        context,
        false,
    )
    .await?;
    let operation = prepared.operation.as_ref().ok_or(Error::StateUnavailable)?;
    if let Some(token) = store.turn_token(operation) {
        let mut value: Value =
            serde_json::from_slice(&prepared.body).map_err(|_| Error::InvalidRequest)?;
        value["client_metadata"]["x-codex-turn-state"] = Value::String(token.clone());
        prepared.headers.insert(
            "x-codex-turn-state",
            token.parse().map_err(|_| Error::InvalidRequest)?,
        );
        prepared.body = Bytes::from(serde_json::to_vec(&value).map_err(|_| Error::InvalidRequest)?);
        if prepared.body.len() > assembly_limit {
            return Err(Error::InvalidRequest);
        }
    }
    Ok(prepared)
}
