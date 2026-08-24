use crate::error::CoreFailure;
use bytes::Bytes;
use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use reqwest::Client;
use reqwest::Request;
use std::io::Cursor;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::extensions::ExtensionsConfig;
use tokio_tungstenite::tungstenite::extensions::compression::deflate::DeflateConfig;
use tokio_tungstenite::tungstenite::handshake::client::Request as WebSocketRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use url::Url;

pub(crate) const CODEX_COMPATIBILITY_VERSION: &str = "0.149.0";
pub(crate) const CODEX_VERSION_HEADER: &str = "version";
pub(crate) const RESPONSES_WEBSOCKET_BETA: &str = "responses_websockets=2026-02-06";
pub(crate) const CODEX_ROUTING_HINT_HEADER: &str = "x-codex-routing-hint";
pub(crate) const DEFAULT_CODEX_ORIGINATOR: &str = "codex_cli_rs";

const COMMON_ALLOWED: &[&str] = &[
    CODEX_VERSION_HEADER,
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
    "x-codex-inference-call-id",
    CODEX_ROUTING_HINT_HEADER,
    "x-codex-turn-state",
    "x-codex-turn-metadata",
    "x-codex-parent-thread-id",
    "x-openai-subagent",
    "x-codex-window-id",
    "x-codex-installation-id",
    "x-openai-internal-codex-responses-lite",
    "x-openai-internal-codex-residency",
    "x-openai-memgen-request",
    "x-oai-attestation",
    "x-responsesapi-include-timing-metrics",
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

const HTTP_HEADER_ORDER: &[&str] = &[
    CODEX_VERSION_HEADER,
    "x-openai-internal-codex-residency",
    "x-codex-beta-features",
    "x-codex-turn-state",
    "x-codex-window-id",
    "x-codex-turn-metadata",
    "x-codex-parent-thread-id",
    "x-openai-subagent",
    "x-openai-memgen-request",
    "x-oai-attestation",
    "x-codex-installation-id",
    "x-openai-internal-codex-responses-lite",
    CODEX_ROUTING_HINT_HEADER,
    "x-codex-inference-call-id",
    "x-client-request-id",
    "session-id",
    "thread-id",
    "accept",
    "content-encoding",
    "content-type",
    "authorization",
    "chatgpt-account-id",
    "originator",
    "user-agent",
    "x-responsesapi-include-timing-metrics",
    "openai-beta",
    "session_id",
    "conversation_id",
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

const WEBSOCKET_WIRE_HEADER_ORDER: &[&str] = &[
    "authorization",
    "chatgpt-account-id",
    "user-agent",
    "x-responsesapi-include-timing-metrics",
    "x-openai-internal-codex-residency",
    "x-openai-memgen-request",
    "x-oai-attestation",
    "originator",
    "openai-beta",
    "x-codex-turn-metadata",
    "x-openai-subagent",
    CODEX_VERSION_HEADER,
    "x-codex-installation-id",
    "x-codex-beta-features",
    CODEX_ROUTING_HINT_HEADER,
    "x-codex-inference-call-id",
    "x-client-request-id",
    "session-id",
    "thread-id",
    "x-codex-window-id",
    "x-codex-turn-state",
    "x-codex-parent-thread-id",
    "x-openai-internal-codex-responses-lite",
    "session_id",
    "conversation_id",
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

const WEBSOCKET_SUBAGENT_WIRE_HEADER_ORDER: &[&str] = &[
    "authorization",
    "chatgpt-account-id",
    "user-agent",
    "x-responsesapi-include-timing-metrics",
    "x-openai-internal-codex-residency",
    "x-openai-memgen-request",
    "x-oai-attestation",
    "originator",
    "openai-beta",
    "x-openai-subagent",
    CODEX_VERSION_HEADER,
    "x-codex-installation-id",
    "x-codex-beta-features",
    CODEX_ROUTING_HINT_HEADER,
    "x-codex-inference-call-id",
    "x-client-request-id",
    "session-id",
    "thread-id",
    "x-codex-window-id",
    "x-codex-turn-metadata",
    "x-codex-parent-thread-id",
    "x-codex-turn-state",
    "x-openai-internal-codex-responses-lite",
    "session_id",
    "conversation_id",
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

const OAUTH_WEBSOCKET_WIRE_HEADER_ORDER: &[&str] = &[
    "chatgpt-account-id",
    "authorization",
    "user-agent",
    "x-oai-attestation",
    "openai-beta",
    CODEX_ROUTING_HINT_HEADER,
    CODEX_VERSION_HEADER,
    "x-openai-internal-codex-residency",
    "x-codex-beta-features",
    "originator",
    "x-client-request-id",
    "session-id",
    "thread-id",
    "x-codex-window-id",
    "x-codex-turn-metadata",
    "x-codex-parent-thread-id",
    "x-openai-subagent",
    "x-openai-memgen-request",
    "x-codex-turn-state",
    "x-codex-installation-id",
    "x-openai-internal-codex-responses-lite",
    "session_id",
    "conversation_id",
];

const OAUTH_WEBSOCKET_TIMING_WIRE_HEADER_ORDER: &[&str] = &[
    "chatgpt-account-id",
    "authorization",
    "user-agent",
    "x-oai-attestation",
    "x-responsesapi-include-timing-metrics",
    "openai-beta",
    CODEX_VERSION_HEADER,
    "x-openai-internal-codex-residency",
    "x-codex-beta-features",
    "originator",
    "x-client-request-id",
    "session-id",
    "thread-id",
    "x-codex-window-id",
    "x-codex-turn-metadata",
    "x-codex-parent-thread-id",
    "x-openai-subagent",
    "x-openai-memgen-request",
    CODEX_ROUTING_HINT_HEADER,
    "x-codex-turn-state",
    "x-codex-installation-id",
    "x-openai-internal-codex-responses-lite",
    "session_id",
    "conversation_id",
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
    let mut headers = authenticated_headers(inbound_headers, auth)?;
    if matches!(auth, ResolvedAuth::CodexOAuth { .. }) {
        headers.insert(
            http::header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.remove("openai-beta");
        headers.remove("x-responsesapi-include-timing-metrics");
    }
    let body = prepare_http_body(&mut headers, auth, body)?;
    let headers = ordered_headers(&headers, HTTP_HEADER_ORDER);
    client
        .post(upstream_url)
        .headers(headers)
        .body(body)
        .build()
        .map_err(|_| CoreFailure::UpstreamConnectFailed)
}

pub(crate) fn build_websocket(
    inbound_headers: &HeaderMap,
    upstream_url: &str,
    auth: &ResolvedAuth,
    max_message_bytes: usize,
) -> Result<(WebSocketRequest, WebSocketConfig), CoreFailure> {
    let mut headers = authenticated_headers(inbound_headers, auth)?;
    headers.remove(http::header::ACCEPT);
    headers.remove(http::header::CONTENT_ENCODING);
    headers.remove(http::header::CONTENT_TYPE);
    if matches!(auth, ResolvedAuth::CodexOAuth { .. }) {
        headers.remove("x-codex-turn-state");
        headers.remove("x-codex-inference-call-id");
        headers.remove("x-openai-internal-codex-responses-lite");
    }
    headers.insert(
        "openai-beta",
        HeaderValue::from_static(RESPONSES_WEBSOCKET_BETA),
    );
    let url = websocket_url(upstream_url)?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| CoreFailure::UpstreamConnectFailed)?;
    insert_websocket_headers(request.headers_mut(), &headers);

    let mut extensions = ExtensionsConfig::default();
    extensions.permessage_deflate = Some(DeflateConfig::default());
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(max_message_bytes);
    config.max_frame_size = Some(max_message_bytes);
    config.extensions = extensions;
    Ok((request, config))
}

fn ordered_headers(source: &HeaderMap, order: &[&'static str]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for name in order {
        let name = HeaderName::from_static(name);
        for value in source.get_all(&name) {
            headers.append(name.clone(), value.clone());
        }
    }
    headers
}

fn insert_websocket_headers(destination: &mut HeaderMap, source: &HeaderMap) {
    let order = if source.contains_key("chatgpt-account-id")
        && source.contains_key("x-responsesapi-include-timing-metrics")
    {
        OAUTH_WEBSOCKET_TIMING_WIRE_HEADER_ORDER
    } else if source.contains_key("chatgpt-account-id") {
        OAUTH_WEBSOCKET_WIRE_HEADER_ORDER
    } else if source.contains_key("x-openai-subagent") {
        WEBSOCKET_SUBAGENT_WIRE_HEADER_ORDER
    } else {
        WEBSOCKET_WIRE_HEADER_ORDER
    };
    let desired = order
        .iter()
        .filter_map(|name| {
            source
                .get(*name)
                .map(|value| (HeaderName::from_static(name), value.clone()))
        })
        .collect::<Vec<_>>();

    // The pinned tungstenite generator removes five mandatory headers with HeaderMap::remove.
    // HeaderMap uses swap-remove, so those removals move the last five custom entries to the
    // front in reverse order. Pre-rotate the entries so their final wire order matches Codex.
    let moved = desired.len().min(5);
    for (name, value) in desired[moved..].iter().chain(desired[..moved].iter().rev()) {
        destination.insert(name.clone(), value.clone());
    }
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
            pin_codex_originator(&mut headers);
            for name in ["x-codex-installation-id", "session_id", "conversation_id"] {
                headers.remove(name);
            }
            headers.insert(
                CODEX_VERSION_HEADER,
                HeaderValue::from_static(CODEX_COMPATIBILITY_VERSION),
            );
            crate::codex_user_agent::pin(&mut headers)?;
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

fn pin_codex_originator(headers: &mut HeaderMap) {
    let valid = headers
        .get("originator")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let value = value.trim();
            value.starts_with("codex") || value.starts_with("Codex ")
        });
    if !valid {
        headers.insert(
            "originator",
            HeaderValue::from_static(DEFAULT_CODEX_ORIGINATOR),
        );
    }
}

fn prepare_http_body(
    headers: &mut HeaderMap,
    auth: &ResolvedAuth,
    body: Bytes,
) -> Result<Bytes, CoreFailure> {
    if !matches!(auth, ResolvedAuth::CodexOAuth { .. }) {
        return Ok(body);
    }
    let encoding = headers
        .get(http::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match encoding {
        Some(value) if value.eq_ignore_ascii_case("zstd") => Ok(body),
        Some(value) if !value.eq_ignore_ascii_case("identity") => Err(CoreFailure::InvalidRequest),
        _ => {
            let compressed = zstd::stream::encode_all(Cursor::new(body.as_ref()), 3)
                .map_err(|_| CoreFailure::Internal)?;
            headers.insert(
                http::header::CONTENT_ENCODING,
                HeaderValue::from_static("zstd"),
            );
            Ok(Bytes::from(compressed))
        }
    }
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

#[cfg(test)]
#[path = "upstream_request_oauth_wire_tests.rs"]
mod oauth_wire_tests;
