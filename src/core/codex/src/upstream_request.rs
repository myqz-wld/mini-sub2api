use crate::error::CoreFailure;
use bytes::Bytes;
use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use reqwest::Client;
use reqwest::Request;
use reqwest_websocket::RequestBuilderExt;
use reqwest_websocket::UpgradedRequestBuilder;
use tungstenite::protocol::WebSocketConfig;
use url::Url;

pub(crate) const CODEX_COMPATIBILITY_VERSION: &str = "0.147.0";
pub(crate) const RESPONSES_WEBSOCKET_BETA: &str = "responses_websockets=2026-02-06";

const COMMON_ALLOWED: &[&str] = &[
    "accept",
    "content-encoding",
    "content-type",
    "originator",
    "session-id",
    "thread-id",
    "user-agent",
    "openai-beta",
    "x-client-request-id",
    "x-codex-beta-features",
    "x-codex-turn-state",
    "x-codex-turn-metadata",
    "x-codex-parent-thread-id",
    "x-codex-window-id",
    "x-codex-installation-id",
    "x-openai-internal-codex-responses-lite",
    "session_id",
    "conversation_id",
];

const OPENAI_API_KEY_ALLOWED: &[&str] = &[
    "openai-organization",
    "openai-project",
    "x-stainless-arch",
    "x-stainless-lang",
    "x-stainless-os",
    "x-stainless-package-version",
    "x-stainless-retry-count",
    "x-stainless-runtime",
    "x-stainless-runtime-version",
    "x-stainless-timeout",
];

#[derive(Clone)]
pub(crate) enum ResolvedAuth {
    CodexOAuth { token: String, account_id: String },
    OpenAiApiKey { token: String },
}

pub(crate) fn build(
    client: &Client,
    inbound_headers: &HeaderMap,
    upstream_url: &str,
    auth: &ResolvedAuth,
    body: Bytes,
) -> Result<Request, CoreFailure> {
    let headers = authenticated_headers(inbound_headers, auth)?;
    client
        .post(upstream_url)
        .headers(headers)
        .body(body)
        .build()
        .map_err(|_| CoreFailure::UpstreamConnectFailed)
}

pub(crate) fn build_websocket(
    client: &Client,
    inbound_headers: &HeaderMap,
    upstream_url: &str,
    auth: &ResolvedAuth,
    max_message_bytes: usize,
) -> Result<UpgradedRequestBuilder, CoreFailure> {
    let mut headers = authenticated_headers(inbound_headers, auth)?;
    headers.remove(http::header::ACCEPT);
    headers.remove(http::header::CONTENT_ENCODING);
    headers.remove(http::header::CONTENT_TYPE);
    headers.insert(
        "openai-beta",
        HeaderValue::from_static(RESPONSES_WEBSOCKET_BETA),
    );
    let url = websocket_url(upstream_url)?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(max_message_bytes))
        .max_frame_size(Some(max_message_bytes));
    Ok(client
        .get(url)
        .version(reqwest::Version::HTTP_11)
        .headers(headers)
        .upgrade()
        .web_socket_config(config))
}

fn authenticated_headers(
    inbound_headers: &HeaderMap,
    auth: &ResolvedAuth,
) -> Result<HeaderMap, CoreFailure> {
    let mut headers = forwarded_headers(inbound_headers, auth);
    let token = match auth {
        ResolvedAuth::CodexOAuth { token, account_id } => {
            let value = HeaderValue::from_str(account_id).map_err(|_| CoreFailure::Internal)?;
            headers.insert("chatgpt-account-id", value);
            if !headers.contains_key("originator") {
                headers.insert("originator", HeaderValue::from_static("mini_sub2api"));
            }
            pin_codex_user_agent(&mut headers)?;
            token
        }
        ResolvedAuth::OpenAiApiKey { token } => token,
    };
    let mut authorization =
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| CoreFailure::Internal)?;
    authorization.set_sensitive(true);
    headers.insert(http::header::AUTHORIZATION, authorization);
    Ok(headers)
}

pub(crate) fn websocket_url(upstream_url: &str) -> Result<Url, CoreFailure> {
    let mut url = Url::parse(upstream_url).map_err(|_| CoreFailure::UpstreamConnectFailed)?;
    let websocket_scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        "ws" | "wss" => return Ok(url),
        _ => return Err(CoreFailure::UpstreamConnectFailed),
    };
    url.set_scheme(websocket_scheme)
        .map_err(|_| CoreFailure::UpstreamConnectFailed)?;
    Ok(url)
}

fn pin_codex_user_agent(headers: &mut HeaderMap) -> Result<(), CoreFailure> {
    let anchored = headers
        .get(http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .and_then(anchor_existing_codex_user_agent)
        .unwrap_or_else(|| format!("codex_cli_rs/{CODEX_COMPATIBILITY_VERSION}"));
    let value = HeaderValue::from_str(&anchored).map_err(|_| CoreFailure::Internal)?;
    headers.insert(http::header::USER_AGENT, value);
    Ok(())
}

fn anchor_existing_codex_user_agent(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let (product_and_version, suffix) = raw
        .split_once(' ')
        .map_or((raw, ""), |(token, suffix)| (token, suffix));
    let (product, version) = product_and_version.split_once('/')?;
    let normalized_product = product.to_ascii_lowercase();
    if version.is_empty()
        || !(normalized_product == "codex"
            || normalized_product.starts_with("codex_")
            || normalized_product.starts_with("codex-"))
    {
        return None;
    }
    let suffix = if suffix.is_empty() {
        String::new()
    } else {
        format!(" {suffix}")
    };
    Some(format!("{product}/{CODEX_COMPATIBILITY_VERSION}{suffix}"))
}

fn forwarded_headers(source: &HeaderMap, auth: &ResolvedAuth) -> HeaderMap {
    let mut headers = HeaderMap::new();
    copy_allowed(&mut headers, source, COMMON_ALLOWED);
    if matches!(auth, ResolvedAuth::OpenAiApiKey { .. }) {
        copy_allowed(&mut headers, source, OPENAI_API_KEY_ALLOWED);
    }
    headers
}

fn copy_allowed(destination: &mut HeaderMap, source: &HeaderMap, allowed: &[&'static str]) {
    for name in allowed {
        if let Some(value) = source.get(*name) {
            destination.insert(HeaderName::from_static(name), value.clone());
        }
    }
}

#[cfg(test)]
#[path = "upstream_request_tests.rs"]
mod tests;
