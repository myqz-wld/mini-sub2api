use crate::error::CoreFailure;
use crate::fingerprint::FingerprintMode;
use crate::fingerprint::FingerprintSnapshot;
use crate::fingerprint_projection::project_http_device;
use crate::http_body::decode_emulated_request_body;
use crate::inference_fingerprint::headers_for_retry;
#[path = "server_internal_request.rs"]
mod internal_request;
use crate::oauth::OAuthFailure;
use crate::oauth::access_token_and_account;
use crate::oauth::refresh_if_needed;
use crate::request_normalizer::EmulationTransport;
use crate::request_normalizer::StatefulPrepareError;
use crate::request_normalizer::SubscriptionStateContext;
use crate::request_normalizer::prepare_emulated_request;
use crate::request_normalizer::prepare_stateful_subscription_request;
use crate::request_profile::CallerKind;
use crate::request_profile::UpstreamProfile;
use crate::response_stream::build_http_response;
use crate::response_stream::request_expects_sse;
use crate::response_translation::ResponseStateContext;
use crate::responses_websocket::responses_socket;
use crate::transport_registry::CredentialTransportContext;
use crate::transport_registry::CredentialTransportPolicy;
use crate::transport_registry::TransportRegistry;
use crate::upstream_request::ResolvedAuth;
use crate::upstream_request::build as build_upstream_request;
use crate::vault::CredentialMaterial;
use crate::vault::CredentialStatus;
use crate::vault::Vault;
use anyhow::Context;
use anyhow::Result;
use axum::Router;
use axum::body::Body;
use axum::body::to_bytes;
use axum::extract::ConnectInfo;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::Request;
use axum::http::Response;
use axum::http::StatusCode;
use axum::routing::get;
use axum::routing::post;
use mini_sub2api_protocol_v1::BuildIdentity;
use mini_sub2api_protocol_v1::Capabilities;
use mini_sub2api_protocol_v1::REQUEST_ID_HEADER;
use mini_sub2api_protocol_v1::Readiness;
use mini_sub2api_protocol_v1::VERSION;
#[cfg(test)]
use mini_sub2api_protocol_v1::{ACCOUNT_REF_HEADER, PSEUDONYM_SCOPE_HEADER, VERSION_HEADER};
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashMap;
use std::io::BufRead;
use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

#[cfg(test)]
pub(crate) use internal_request::validate_internal_auth;
pub(crate) use internal_request::validate_internal_request;

const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) vault: Vault,
    pub(crate) transports: Arc<TransportRegistry>,
    pub(crate) internal_token_hash: [u8; 32],
    pub(crate) account_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

pub(crate) struct ResolvedCredential {
    pub(crate) upstream_url: String,
    pub(crate) auth: ResolvedAuth,
    pub(crate) fingerprint: FingerprintSnapshot,
    pub(crate) transport: Arc<CredentialTransportContext>,
}

pub async fn run(listen: SocketAddr, state_dir: PathBuf) -> Result<()> {
    anyhow::ensure!(
        listen.ip().is_loopback(),
        "internal listener must be loopback"
    );
    let token = read_internal_token().await?;
    let vault = Vault::open(state_dir)?;
    let _instance_lock = vault.acquire_instance_lock()?;
    let state = AppState {
        vault,
        transports: Arc::new(TransportRegistry::new()?),
        internal_token_hash: Sha256::digest(token.as_bytes()).into(),
        account_locks: Arc::new(Mutex::new(HashMap::new())),
    };
    drop(token);

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .context("binding internal listener")?;
    let actual = listener.local_addr()?;
    write_readiness(actual.port())?;

    let app = internal_router(state);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("serving internal API")
}

pub(crate) fn internal_router(state: AppState) -> Router {
    Router::new()
        .route("/internal/v1/responses", post(responses))
        .route("/internal/v1/responses/ws", get(responses_socket))
        .with_state(state)
}

async fn responses(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response<Body> {
    let request_id = header_text(&headers, REQUEST_ID_HEADER).unwrap_or_default();
    match responses_inner(peer, &state, headers, request).await {
        Ok(response) => response,
        Err(error) => error.into_response(request_id),
    }
}

async fn responses_inner(
    peer: SocketAddr,
    state: &AppState,
    headers: HeaderMap,
    request: Request<Body>,
) -> std::result::Result<Response<Body>, CoreFailure> {
    let identity = validate_internal_request(peer, state, &headers)?;
    let caller = CallerKind::from_headers(&headers);
    let account_ref = identity.account_ref;
    let pseudonym_scope = identity.pseudonym_scope;
    let body = to_bytes(request.into_body(), MAX_REQUEST_BYTES)
        .await
        .map_err(|_| CoreFailure::InvalidRequest)?;
    let downstream_expects_sse = request_expects_sse(&body);
    let account_lock = account_lock(state, &account_ref).await;

    let _guard = account_lock.lock().await;
    let resolved = resolve_auth(state, &account_ref, None).await?;
    drop(_guard);
    let profile = UpstreamProfile::select(caller, resolved.auth.credential_kind());
    let account_namespace = match &resolved.auth {
        ResolvedAuth::CodexOAuth { account_id, .. } => Some(account_id.clone()),
        ResolvedAuth::OpenAiApiKey { .. } => None,
    };
    let mut forward_headers = headers;
    let body = if profile.emulates_codex() {
        decode_emulated_request_body(&mut forward_headers, body, MAX_REQUEST_BYTES)
            .map_err(|()| CoreFailure::InvalidRequest)?
    } else {
        body
    };
    let (forward_headers, body, resolved_identity) = if profile.emulates_codex() {
        let prepared = if profile.uses_codex_subscription() {
            let account_namespace = account_namespace.as_deref().ok_or(CoreFailure::Internal)?;
            prepare_stateful_subscription_request(
                EmulationTransport::Http,
                &forward_headers,
                body,
                MAX_REQUEST_BYTES,
                SubscriptionStateContext {
                    account_ref: &account_ref,
                    account_namespace,
                    downstream_scope: &pseudonym_scope,
                    fingerprint_mode: resolved.fingerprint.mode(),
                    store: state.vault.request_state(),
                },
                false,
            )
            .await
            .map_err(|error| match error {
                StatefulPrepareError::InvalidRequest => CoreFailure::InvalidRequest,
                StatefulPrepareError::StateUnavailable => CoreFailure::StateUnavailable,
            })?
        } else {
            prepare_emulated_request(
                profile,
                EmulationTransport::Http,
                &forward_headers,
                body,
                MAX_REQUEST_BYTES,
                None,
            )
            .map_err(|()| CoreFailure::InvalidRequest)?
        };
        (prepared.headers, prepared.body, prepared.resolved_identity)
    } else {
        (forward_headers, body, None)
    };
    let (forward_headers, body) = if resolved.fingerprint.mode() == FingerprintMode::Device
        && profile.uses_codex_subscription()
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
            MAX_REQUEST_BYTES,
        )
        .map_err(|_| CoreFailure::InvalidRequest)?;
        (projected.headers, projected.body)
    } else {
        (forward_headers, body)
    };
    let started = Instant::now();
    let mut upstream = send_upstream(
        &resolved.transport,
        &forward_headers,
        &resolved.upstream_url,
        &resolved.auth,
        profile,
        body.clone(),
    )
    .await?;
    if upstream.status() == StatusCode::UNAUTHORIZED
        && matches!(resolved.auth, ResolvedAuth::CodexOAuth { .. })
    {
        let failed_access_token = match &resolved.auth {
            ResolvedAuth::CodexOAuth { token, .. } => token.clone(),
            ResolvedAuth::OpenAiApiKey { .. } => return Err(CoreFailure::Internal),
        };
        let _guard = account_lock.lock().await;
        let retry = resolve_auth(state, &account_ref, Some(&failed_access_token)).await?;
        drop(_guard);
        let retry_headers = headers_for_retry(&forward_headers);
        upstream = send_upstream(
            &retry.transport,
            &retry_headers,
            &retry.upstream_url,
            &retry.auth,
            profile,
            body,
        )
        .await?;
        if upstream.status() == StatusCode::UNAUTHORIZED {
            return Err(CoreFailure::UpstreamAuthFailed);
        }
    }
    let ttfb_ms = started.elapsed().as_millis();
    let response_state = account_namespace.as_deref().and_then(|namespace| {
        profile.uses_codex_subscription().then(|| {
            ResponseStateContext::new(
                &account_ref,
                namespace,
                &pseudonym_scope,
                state.vault.request_state(),
                resolved_identity.as_ref(),
            )
        })
    });
    build_http_response(
        upstream,
        ttfb_ms,
        downstream_expects_sse,
        profile,
        response_state,
    )
    .await
}

pub(crate) async fn resolve_auth(
    state: &AppState,
    account_ref: &str,
    failed_access_token: Option<&str>,
) -> std::result::Result<ResolvedCredential, CoreFailure> {
    let mut locked = state
        .vault
        .lock_record(account_ref)
        .await
        .map_err(|_| CoreFailure::UnknownAccount)?;
    if locked.record.status == CredentialStatus::RequiresLogin {
        return Err(CoreFailure::CredentialRequiresLogin);
    }
    let transport = state
        .transports
        .context(account_ref, CredentialTransportPolicy::default())
        .map_err(|_| CoreFailure::UpstreamConnectFailed)?;
    let fingerprint = locked.fingerprint().clone();
    if matches!(
        locked.record.material,
        CredentialMaterial::CodexOAuth { .. }
    ) {
        let issuer = match &locked.record.material {
            CredentialMaterial::CodexOAuth { issuer, .. } => issuer.clone(),
            CredentialMaterial::OpenAiApiKey { .. } => String::new(),
        };
        let refresh_needed = match (&locked.record.material, failed_access_token) {
            (CredentialMaterial::CodexOAuth { access_token, .. }, Some(failed_access_token)) => {
                access_token == failed_access_token
            }
            (CredentialMaterial::CodexOAuth { .. }, None) => true,
            (CredentialMaterial::OpenAiApiKey { .. }, _) => false,
        };
        if refresh_needed {
            refresh_if_needed(
                &mut locked,
                transport.http_client_for_url(&issuer),
                failed_access_token.is_some(),
            )
            .await
            .map_err(|error| match error {
                OAuthFailure::RequiresLogin => CoreFailure::CredentialRequiresLogin,
                OAuthFailure::Transport(_) => CoreFailure::UpstreamConnectFailed,
            })?;
        }
    }
    let upstream_url = locked.record.upstream_url.clone();
    let auth = if let Some((token, account_id)) = access_token_and_account(&locked.record) {
        ResolvedAuth::CodexOAuth {
            token: token.to_string(),
            account_id: account_id.to_string(),
        }
    } else {
        match &locked.record.material {
            CredentialMaterial::OpenAiApiKey { api_key } => ResolvedAuth::OpenAiApiKey {
                token: api_key.clone(),
            },
            CredentialMaterial::CodexOAuth { .. } => return Err(CoreFailure::Internal),
        }
    };
    Ok(ResolvedCredential {
        upstream_url,
        auth,
        fingerprint,
        transport,
    })
}

async fn send_upstream(
    transport: &CredentialTransportContext,
    inbound_headers: &HeaderMap,
    upstream_url: &str,
    auth: &ResolvedAuth,
    profile: UpstreamProfile,
    body: bytes::Bytes,
) -> std::result::Result<reqwest::Response, CoreFailure> {
    let client = transport.http_client_for_url(upstream_url);
    let request =
        build_upstream_request(client, inbound_headers, upstream_url, auth, profile, body)?;
    client.execute(request).await.map_err(|error| {
        if error.is_connect() {
            CoreFailure::UpstreamConnectFailed
        } else {
            CoreFailure::UpstreamDeliveryUnknown
        }
    })
}

pub(crate) async fn account_lock(state: &AppState, account_ref: &str) -> Arc<Mutex<()>> {
    let mut locks = state.account_locks.lock().await;
    Arc::clone(
        locks
            .entry(account_ref.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}

pub(crate) fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

async fn read_internal_token() -> Result<String> {
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .take(1026)
            .read_line(&mut line)
            .context("reading internal token")?;
        anyhow::ensure!(
            line.ends_with('\n') && line.len() <= 1025,
            "invalid internal token record"
        );
        let token = line.trim().to_string();
        anyhow::ensure!(
            token.len() >= 32 && token.len() <= 1024,
            "invalid internal token"
        );
        Ok(token)
    })
    .await
    .context("internal token read task failed")?
}

fn write_readiness(port: u16) -> Result<()> {
    let readiness = Readiness {
        protocol_version: VERSION.to_string(),
        port,
        pid: std::process::id(),
        build: BuildIdentity {
            name: env!("CARGO_PKG_NAME").to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: option_env!("MINI_SUB2API_BUILD_COMMIT")
                .unwrap_or("unknown")
                .to_string(),
        },
        capabilities: Capabilities {
            responses_web_socket: true,
        },
    };
    let mut output = serde_json::to_string(&readiness)?;
    output.push('\n');
    anyhow::ensure!(output.len() <= 4096, "readiness record is too large");
    std::io::stdout().write_all(output.as_bytes())?;
    std::io::stdout().flush()?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

pub fn parse_internal_listen(raw: &str) -> Result<SocketAddr> {
    let address: SocketAddr = raw.parse().context("parsing internal listen address")?;
    anyhow::ensure!(
        address.ip().is_loopback(),
        "internal listener must be loopback"
    );
    Ok(address)
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "server_integration_support.rs"]
mod integration_support;

#[cfg(test)]
#[path = "server_integration_tests.rs"]
mod integration_tests;

#[cfg(test)]
#[path = "server_fingerprint_tests.rs"]
mod fingerprint_http_tests;

#[cfg(test)]
#[path = "server_oauth_tests.rs"]
mod oauth_integration_tests;

#[cfg(test)]
#[path = "server_compaction_tests.rs"]
mod compaction_integration_tests;
