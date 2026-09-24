#[path = "server_http.rs"]
mod http_forward;
use http_forward::responses_inner;

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
use crate::request_normalizer::CodexStateContext;
use crate::request_normalizer::EmulationTransport;
use crate::request_normalizer::StatefulPrepareError;
use crate::request_normalizer::prepare_stateful_codex_request;
use crate::request_profile::CallerKind;
use crate::request_profile::UpstreamProfile;
use crate::response_stream::build_http_failure_response;
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
    pub(crate) state_namespace: String,
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

pub(crate) fn validate_recovery_owner(
    previous: &ResolvedCredential,
    next: &ResolvedCredential,
) -> std::result::Result<(), CoreFailure> {
    let same_account = matches!((&previous.auth, &next.auth),
        (ResolvedAuth::CodexOAuth { account_id: a, .. }, ResolvedAuth::CodexOAuth { account_id: b, .. }) if a == b);
    if !same_account
        || previous.state_namespace != next.state_namespace
        || previous.upstream_url != next.upstream_url
    {
        return Err(CoreFailure::CredentialRequiresLogin);
    }
    Ok(())
}

pub(crate) async fn resolve_auth(
    state: &AppState,
    account_ref: &str,
    failed_access_token: Option<&str>,
) -> std::result::Result<ResolvedCredential, CoreFailure> {
    resolve_auth_inner(state, account_ref, failed_access_token, false).await
}

pub(crate) async fn reload_auth(
    state: &AppState,
    account_ref: &str,
) -> std::result::Result<ResolvedCredential, CoreFailure> {
    resolve_auth_inner(state, account_ref, None, true).await
}

async fn resolve_auth_inner(
    state: &AppState,
    account_ref: &str,
    failed_access_token: Option<&str>,
    reload_only: bool,
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
        let managed_reload = reload_only
            && matches!(&locked.record.material,
            CredentialMaterial::CodexOAuth { refresh_token, .. } if !refresh_token.is_empty());
        if refresh_needed && !managed_reload {
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
    let state_namespace = locked.record.request_state_namespace().to_string();
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
        state_namespace,
        transport,
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

#[path = "server_listener.rs"]
mod listener;
pub use listener::parse_internal_listen;

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "server_integration_support.rs"]
pub(crate) mod integration_support;

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

#[cfg(test)]
#[path = "server_response_privacy_tests.rs"]
mod response_privacy_tests;

#[cfg(test)]
#[path = "server_reference_tests.rs"]
mod reference_integration_tests;

#[cfg(test)]
#[path = "server_passthrough_tests.rs"]
mod passthrough_tests;

#[cfg(test)]
#[path = "server_context_tests.rs"]
mod context_tests;
