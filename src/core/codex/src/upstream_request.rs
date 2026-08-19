use crate::error::CoreFailure;
use bytes::Bytes;
use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use reqwest::Client;
use reqwest::Request;

pub(crate) const CODEX_COMPATIBILITY_VERSION: &str = "0.147.0";

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
    client
        .post(upstream_url)
        .headers(headers)
        .body(body)
        .build()
        .map_err(|_| CoreFailure::UpstreamConnectFailed)
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
