use crate::error::CoreFailure;
use crate::http_client::has_literal_loopback_host;
use crate::oauth::OAuthFailure;
use crate::oauth::access_token_and_account;
use crate::oauth::refresh_if_needed;
use crate::request_normalizer::prepare_subscription_request;
use crate::responses_websocket::responses_socket;
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
use axum::http::HeaderName;
use axum::http::Request;
use axum::http::Response;
use axum::http::StatusCode;
use axum::routing::get;
use axum::routing::post;
use futures_util::StreamExt;
use mini_sub2api_protocol_v1::ACCOUNT_REF_HEADER;
use mini_sub2api_protocol_v1::BuildIdentity;
use mini_sub2api_protocol_v1::CORE_TTFB_HEADER;
use mini_sub2api_protocol_v1::Capabilities;
use mini_sub2api_protocol_v1::REQUEST_ID_HEADER;
use mini_sub2api_protocol_v1::Readiness;
use mini_sub2api_protocol_v1::VERSION;
use mini_sub2api_protocol_v1::VERSION_HEADER;
use reqwest::Client;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::BufRead;
use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) vault: Vault,
    pub(crate) client: Client,
    pub(crate) direct_client: Client,
    pub(crate) websocket_client: Client,
    pub(crate) direct_websocket_client: Client,
    pub(crate) internal_token_hash: [u8; 32],
    pub(crate) account_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

pub async fn run(listen: SocketAddr, state_dir: PathBuf) -> Result<()> {
    anyhow::ensure!(
        listen.ip().is_loopback(),
        "internal listener must be loopback"
    );
    let token = read_internal_token().await?;
    let vault = Vault::open(state_dir)?;
    let _instance_lock = vault.acquire_instance_lock()?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("building upstream client")?;
    let direct_client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .context("building direct loopback client")?;
    let websocket_client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .http1_only()
        .build()
        .context("building upstream WebSocket client")?;
    let direct_websocket_client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .http1_only()
        .no_proxy()
        .build()
        .context("building direct loopback WebSocket client")?;
    let state = AppState {
        vault,
        client,
        direct_client,
        websocket_client,
        direct_websocket_client,
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
    let account_ref = identity.account_ref;
    let request_id = identity.request_id;
    let body = to_bytes(request.into_body(), MAX_REQUEST_BYTES)
        .await
        .map_err(|_| CoreFailure::InvalidRequest)?;
    let account_lock = account_lock(state, &account_ref).await;

    let _guard = account_lock.lock().await;
    let (upstream_url, auth) = resolve_auth(state, &account_ref, None).await?;
    drop(_guard);
    let (forward_headers, body) = if matches!(auth, ResolvedAuth::CodexOAuth { .. }) {
        let prepared = prepare_subscription_request(
            &headers,
            body,
            MAX_REQUEST_BYTES,
            &account_ref,
            &request_id,
        );
        (prepared.headers, prepared.body)
    } else {
        (headers, body)
    };
    let expects_sse = request_expects_sse(&body);

    let started = Instant::now();
    let mut upstream =
        send_upstream(state, &forward_headers, &upstream_url, &auth, body.clone()).await?;
    if upstream.status() == StatusCode::UNAUTHORIZED
        && matches!(auth, ResolvedAuth::CodexOAuth { .. })
    {
        let failed_access_token = match &auth {
            ResolvedAuth::CodexOAuth { token, .. } => token.clone(),
            ResolvedAuth::OpenAiApiKey { .. } => return Err(CoreFailure::Internal),
        };
        let _guard = account_lock.lock().await;
        let (retry_url, retry_auth) =
            resolve_auth(state, &account_ref, Some(&failed_access_token)).await?;
        drop(_guard);
        upstream = send_upstream(state, &forward_headers, &retry_url, &retry_auth, body).await?;
        if upstream.status() == StatusCode::UNAUTHORIZED {
            return Err(CoreFailure::UpstreamAuthFailed);
        }
    }
    let ttfb_ms = started.elapsed().as_millis();
    build_streaming_response(upstream, ttfb_ms, expects_sse)
}

pub(crate) async fn resolve_auth(
    state: &AppState,
    account_ref: &str,
    failed_access_token: Option<&str>,
) -> std::result::Result<(String, ResolvedAuth), CoreFailure> {
    let mut locked = state
        .vault
        .lock_record(account_ref)
        .await
        .map_err(|_| CoreFailure::UnknownAccount)?;
    if locked.record.status == CredentialStatus::RequiresLogin {
        return Err(CoreFailure::CredentialRequiresLogin);
    }
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
                state.client_for_url(&issuer),
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
    Ok((upstream_url, auth))
}

async fn send_upstream(
    state: &AppState,
    inbound_headers: &HeaderMap,
    upstream_url: &str,
    auth: &ResolvedAuth,
    body: bytes::Bytes,
) -> std::result::Result<reqwest::Response, CoreFailure> {
    let client = state.client_for_url(upstream_url);
    let request = build_upstream_request(client, inbound_headers, upstream_url, auth, body)?;
    client
        .execute(request)
        .await
        .map_err(|_| CoreFailure::UpstreamConnectFailed)
}

impl AppState {
    pub(crate) fn client_for_url(&self, url: &str) -> &Client {
        if has_literal_loopback_host(url) {
            &self.direct_client
        } else {
            &self.client
        }
    }

    pub(crate) fn websocket_client_for_url(&self, url: &str) -> &Client {
        if has_literal_loopback_host(url) {
            &self.direct_websocket_client
        } else {
            &self.websocket_client
        }
    }
}

fn build_streaming_response(
    upstream: reqwest::Response,
    ttfb_ms: u128,
    expects_sse: bool,
) -> std::result::Result<Response<Body>, CoreFailure> {
    let status = upstream.status();
    let mut builder = Response::builder().status(status);
    let connection_headers = nominated_connection_headers(upstream.headers());
    let has_content_type = upstream.headers().contains_key(http::header::CONTENT_TYPE);
    for (name, value) in upstream.headers() {
        if is_safe_response_header(name) && !connection_headers.contains(name.as_str()) {
            builder = builder.header(name, value);
        }
    }
    if expects_sse && status.is_success() && !has_content_type {
        builder = builder.header(http::header::CONTENT_TYPE, "text/event-stream");
    }
    builder = builder.header(CORE_TTFB_HEADER, ttfb_ms.to_string());
    let stream = upstream
        .bytes_stream()
        .map(|result| result.map_err(std::io::Error::other));
    builder
        .body(Body::from_stream(stream))
        .map_err(|_| CoreFailure::Internal)
}

fn request_expects_sse(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("stream").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

fn nominated_connection_headers(headers: &HeaderMap) -> HashSet<String> {
    headers
        .get_all(http::header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn validate_internal_auth(
    headers: &HeaderMap,
    expected_hash: &[u8; 32],
) -> std::result::Result<(), CoreFailure> {
    let token = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(CoreFailure::InvalidInternalAuth)?;
    let actual: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    if actual.ct_eq(expected_hash).into() {
        Ok(())
    } else {
        Err(CoreFailure::InvalidInternalAuth)
    }
}

pub(crate) struct InternalRequestIdentity {
    pub account_ref: String,
    pub request_id: String,
}

pub(crate) fn validate_internal_request(
    peer: SocketAddr,
    state: &AppState,
    headers: &HeaderMap,
) -> std::result::Result<InternalRequestIdentity, CoreFailure> {
    if !peer.ip().is_loopback() {
        return Err(CoreFailure::InvalidInternalAuth);
    }
    if header_text(headers, VERSION_HEADER).as_deref() != Some(VERSION) {
        return Err(CoreFailure::UnsupportedProtocol);
    }
    validate_internal_auth(headers, &state.internal_token_hash)?;
    let account_ref = header_text(headers, ACCOUNT_REF_HEADER)
        .filter(|value| value.starts_with("acct_") && value.len() <= 133)
        .ok_or(CoreFailure::InvalidRequest)?;
    let request_id = header_text(headers, REQUEST_ID_HEADER)
        .filter(|value| value.starts_with("req_") && value.len() <= 132)
        .ok_or(CoreFailure::InvalidRequest)?;
    Ok(InternalRequestIdentity {
        account_ref,
        request_id,
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

fn is_safe_response_header(name: &HeaderName) -> bool {
    if name.as_str().starts_with("x-mini-sub2api-") {
        return false;
    }
    !matches!(
        name.as_str(),
        "connection"
            | "content-length"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "set-cookie"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
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
#[path = "server_integration_tests.rs"]
mod integration_tests;
