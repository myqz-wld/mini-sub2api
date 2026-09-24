use super::*;
use crate::error_diagnostics::ErrorDetails;
use crate::request_diagnostics::HttpTrace;
use tracing::Instrument;

pub(super) async fn responses_inner(
    peer: SocketAddr,
    state: &AppState,
    headers: HeaderMap,
    request: Request<Body>,
) -> std::result::Result<Response<Body>, CoreFailure> {
    let identity = validate_internal_request(peer, state, &headers)?;
    let gateway_request_id =
        header_text(&headers, REQUEST_ID_HEADER).ok_or(CoreFailure::InvalidRequest)?;
    let span = tracing::info_span!(
        "http_request",
        request_id = gateway_request_id,
        profile = tracing::field::Empty,
        requested_model = tracing::field::Empty,
        requested_effort = tracing::field::Empty,
        effective_model = tracing::field::Empty,
        effective_effort = tracing::field::Empty
    );
    let mut diagnostics = HttpTrace::new(&gateway_request_id);
    let result = async {
        let caller = CallerKind::from_headers(&headers);
        let account_ref = identity.account_ref;
        let pseudonym_scope = identity.pseudonym_scope;
        let body = to_bytes(
            request.into_body(),
            crate::inference_limits::get().request_bytes,
        )
        .await
        .map_err(|_| CoreFailure::InvalidRequest)?;
        let account_lock = account_lock(state, &account_ref).await;

        let _guard = diagnostics
            .wait("credential_lock", account_lock.lock())
            .await;
        let mut resolved = diagnostics
            .wait(
                "credential_resolution",
                resolve_auth(state, &account_ref, None),
            )
            .await?;
        drop(_guard);
        let profile = UpstreamProfile::select(caller, resolved.auth.credential_kind());
        tracing::Span::current().record(
            "profile",
            if profile.emulates_codex() {
                "subscription"
            } else {
                "api_key"
            },
        );
        let state_namespace = resolved.state_namespace.clone();
        let mut forward_headers = headers;
        let body = if profile.emulates_codex() {
            decode_emulated_request_body(
                &mut forward_headers,
                body,
                crate::inference_limits::get().request_bytes,
            )
            .map_err(|()| CoreFailure::InvalidRequest)?
        } else {
            body
        };
        let downstream_expects_sse = request_expects_sse(&body);
        diagnostics.phase("request_normalization");
        let (forward_headers, body, resolved_identity, pending_compaction, operation) =
            if profile.emulates_codex() {
                let prepared = diagnostics
                    .wait(
                        "request_normalization",
                        prepare_stateful_codex_request(
                            profile,
                            EmulationTransport::Http,
                            &forward_headers,
                            body,
                            crate::inference_limits::get().request_bytes,
                            CodexStateContext {
                                force_lite: false,
                                admission: None,
                                binding: None,
                                socket_id: None,
                                account_ref: &account_ref,
                                state_namespace: &state_namespace,
                                downstream_scope: &pseudonym_scope,
                                fingerprint_mode: resolved.fingerprint.mode(),
                                store: state.vault.request_state(),
                            },
                            false,
                        ),
                    )
                    .await
                    .map_err(|error| match error {
                        StatefulPrepareError::InvalidRequest => CoreFailure::InvalidRequest,
                        StatefulPrepareError::StateUnavailable => CoreFailure::StateUnavailable,
                    })?;
                (
                    prepared.headers,
                    prepared.body,
                    prepared.resolved_identity,
                    prepared.pending_compaction,
                    prepared.operation,
                )
            } else {
                (forward_headers, body, None, None, None)
            };
        let (forward_headers, body) = if resolved.fingerprint.mode() == FingerprintMode::Device
            && profile.uses_identity_state()
        {
            let installation_id = resolved_identity
                .as_ref()
                .map(|identity| identity.installation_id.as_str())
                .ok_or(CoreFailure::Internal)?;
            let projected = project_http_device(
                forward_headers,
                body,
                &resolved.fingerprint,
                installation_id,
                crate::inference_limits::get().request_bytes,
            )
            .map_err(|_| CoreFailure::InvalidRequest)?;
            (projected.headers, projected.body)
        } else {
            (forward_headers, body)
        };
        let persistent = !profile.emulates_codex()
            || forward_headers
                .get("x-codex-guardian")
                .is_some_and(|v| v == "classifier");
        let client = resolved
            .transport
            .inference_client(&resolved.upstream_url, persistent)
            .map_err(|_| CoreFailure::UpstreamConnectFailed)?;
        let started = Instant::now();
        let mut upstream = diagnostics
            .wait(
                "upstream_request",
                send_upstream(
                    &client,
                    &forward_headers,
                    &resolved.upstream_url,
                    &resolved.auth,
                    profile,
                    body.clone(),
                    &gateway_request_id,
                ),
            )
            .await?;
        tracing::info!(
            event = "upstream_headers",
            request_id = gateway_request_id,
            attempt = 1,
            http_status = upstream.status().as_u16(),
            elapsed_ms = started.elapsed().as_millis() as u64
        );
        // Managed native recovery is bounded: reload (even unchanged), then refresh.
        for (step, phase) in ["credential_reload", "credential_refresh"]
            .into_iter()
            .enumerate()
        {
            if upstream.status() != StatusCode::UNAUTHORIZED || !profile.uses_oauth_refresh() {
                break;
            }
            let failed_access_token = match &resolved.auth {
                ResolvedAuth::CodexOAuth { token, .. } => token.clone(),
                ResolvedAuth::OpenAiApiKey { .. } => return Err(CoreFailure::Internal),
            };
            tracing::info!(
                event = "upstream_auth_retry",
                request_id = gateway_request_id,
                attempt = step + 2,
                phase
            );
            let guard = diagnostics
                .wait("credential_refresh_lock", account_lock.lock())
                .await;
            let retry = if step == 0 {
                diagnostics
                    .wait(phase, crate::server::reload_auth(state, &account_ref))
                    .await?
            } else {
                diagnostics
                    .wait(
                        phase,
                        resolve_auth(state, &account_ref, Some(&failed_access_token)),
                    )
                    .await?
            };
            drop(guard);
            crate::server::validate_recovery_owner(&resolved, &retry)?;
            let client = retry
                .transport
                .inference_client(&retry.upstream_url, persistent)
                .map_err(|_| CoreFailure::UpstreamConnectFailed)?;
            let retry_headers = headers_for_retry(&forward_headers);
            upstream = diagnostics
                .wait(
                    "upstream_request",
                    send_upstream(
                        &client,
                        &retry_headers,
                        &retry.upstream_url,
                        &retry.auth,
                        profile,
                        body.clone(),
                        &gateway_request_id,
                    ),
                )
                .await?;
            resolved = retry;
            tracing::info!(
                event = "upstream_headers",
                request_id = gateway_request_id,
                attempt = step + 2,
                http_status = upstream.status().as_u16(),
                elapsed_ms = started.elapsed().as_millis() as u64
            );
        }
        if upstream.status() == StatusCode::UNAUTHORIZED && profile.uses_oauth_refresh() {
            return build_http_failure_response(
                upstream,
                started.elapsed().as_millis(),
                &gateway_request_id,
                &CoreFailure::UpstreamAuthFailed,
            );
        }
        let ttfb_ms = started.elapsed().as_millis();
        diagnostics.phase("response_state");
        if let Some(operation) = &operation
            && let Some(token) = upstream
                .headers()
                .get("x-codex-turn-state")
                .and_then(|v| v.to_str().ok())
        {
            state
                .vault
                .request_state()
                .contexts
                .learn_turn(operation, token)
                .map_err(|_| CoreFailure::StateUnavailable)?;
        }
        let response_state = profile.uses_identity_state().then(|| {
            ResponseStateContext::new(
                &account_ref,
                &state_namespace,
                &pseudonym_scope,
                state.vault.request_state(),
                resolved_identity.as_ref(),
                pending_compaction.as_ref(),
            )
            .with_gateway_request_id(&gateway_request_id)
            .with_operation(operation)
        });
        diagnostics.phase("response_construction");
        build_http_response(
            upstream,
            ttfb_ms,
            downstream_expects_sse,
            profile,
            response_state,
            &gateway_request_id,
        )
        .await
    }
    .instrument(span)
    .await;
    diagnostics.finish(&result);
    result
}

async fn send_upstream(
    client: &reqwest::Client,
    inbound_headers: &HeaderMap,
    upstream_url: &str,
    auth: &ResolvedAuth,
    profile: UpstreamProfile,
    body: bytes::Bytes,
    request_id: &str,
) -> std::result::Result<reqwest::Response, CoreFailure> {
    let request =
        build_upstream_request(client, inbound_headers, upstream_url, auth, profile, body)?;
    client.execute(request).await.map_err(|error| {
        ErrorDetails::observe(&error).log(
            request_id,
            if error.is_connect() {
                "upstream_connect"
            } else {
                "upstream_request"
            },
        );
        if error.is_connect() {
            CoreFailure::UpstreamConnectFailed
        } else {
            CoreFailure::UpstreamDeliveryUnknown
        }
    })
}
