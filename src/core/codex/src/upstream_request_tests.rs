use super::*;
use crate::vault::DEFAULT_OPENAI_RESPONSES_URL;
use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

const OFFICIAL_SDK_BODY: &[u8] = br#"{"input":"offline parity check","model":"gpt-5.2","reasoning":{"effort":"none"},"text":{"verbosity":"low"}}"#;

fn official_sdk_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("accept", "application/json"),
        ("authorization", "Bearer downstream-key"),
        ("content-type", "application/json"),
        ("user-agent", "OpenAI/Go 3.52.0"),
        ("openai-organization", "org-test"),
        ("openai-project", "proj-test"),
        ("x-stainless-arch", "arm64"),
        ("x-stainless-lang", "go"),
        ("x-stainless-os", "MacOS"),
        ("x-stainless-package-version", "3.52.0"),
        ("x-stainless-retry-count", "0"),
        ("x-stainless-runtime", "go"),
        ("x-stainless-runtime-version", "go1.26.0"),
        ("x-stainless-timeout", "30"),
        ("x-forwarded-for", "203.0.113.1"),
        ("x-stainless-unreviewed", "must-not-cross"),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    headers
}

#[test]
fn api_key_request_matches_official_sdk_capture_without_network() {
    // Request body and default transport metadata were captured from openai-go v3.52.0 through
    // an in-memory RoundTripper. Building this reqwest Request opens no socket and sends nothing.
    let client = Client::builder()
        .no_proxy()
        .build()
        .expect("offline client");
    let body = Bytes::from_static(OFFICIAL_SDK_BODY);
    let request = build(
        &client,
        &official_sdk_headers(),
        DEFAULT_OPENAI_RESPONSES_URL,
        &ResolvedAuth::OpenAiApiKey {
            token: "sk-offline-not-real".to_string(),
        },
        body.clone(),
    )
    .expect("offline request build");

    assert_eq!(request.method(), http::Method::POST);
    assert_eq!(request.url().as_str(), DEFAULT_OPENAI_RESPONSES_URL);
    assert_eq!(
        request
            .headers()
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer sk-offline-not-real")
    );
    assert!(
        request
            .headers()
            .get(http::header::AUTHORIZATION)
            .expect("authorization")
            .is_sensitive()
    );
    for (name, expected) in [
        ("accept", "application/json"),
        ("content-type", "application/json"),
        ("user-agent", "OpenAI/Go 3.52.0"),
        ("openai-organization", "org-test"),
        ("openai-project", "proj-test"),
        ("x-stainless-arch", "arm64"),
        ("x-stainless-lang", "go"),
        ("x-stainless-os", "MacOS"),
        ("x-stainless-package-version", "3.52.0"),
        ("x-stainless-retry-count", "0"),
        ("x-stainless-runtime", "go"),
        ("x-stainless-runtime-version", "go1.26.0"),
        ("x-stainless-timeout", "30"),
    ] {
        assert_eq!(
            request
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok()),
            Some(expected),
            "header {name}"
        );
    }
    assert!(!request.headers().contains_key("chatgpt-account-id"));
    assert!(!request.headers().contains_key(CODEX_VERSION_HEADER));
    assert!(!request.headers().contains_key("x-forwarded-for"));
    assert!(!request.headers().contains_key("x-stainless-unreviewed"));
    assert_eq!(
        request.body().and_then(reqwest::Body::as_bytes),
        Some(body.as_ref())
    );
}

#[test]
fn oauth_request_excludes_api_key_routing_and_sdk_headers() {
    let client = Client::builder()
        .no_proxy()
        .build()
        .expect("offline client");
    let request = build(
        &client,
        &official_sdk_headers(),
        "https://chatgpt.com/backend-api/codex/responses",
        &ResolvedAuth::CodexOAuth {
            token: "oauth-offline-not-real".to_string(),
            account_id: "account-test".to_string(),
        },
        Bytes::from_static(OFFICIAL_SDK_BODY),
    )
    .expect("offline request build");

    assert_eq!(
        request
            .headers()
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok()),
        Some("account-test")
    );
    assert_eq!(
        request
            .headers()
            .get("originator")
            .and_then(|value| value.to_str().ok()),
        Some("codex_cli_rs")
    );
    assert!(
        request
            .headers()
            .get(http::header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("codex_cli_rs/0.149.0 ("))
    );
    assert_eq!(
        request
            .headers()
            .get(CODEX_VERSION_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(CODEX_COMPATIBILITY_VERSION)
    );
    for name in OPENAI_API_KEY_ALLOWED {
        assert!(!request.headers().contains_key(*name), "header {name}");
    }
    assert_eq!(
        request
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("zstd")
    );
    let compressed = request
        .body()
        .and_then(reqwest::Body::as_bytes)
        .expect("compressed body");
    assert_eq!(
        zstd::stream::decode_all(std::io::Cursor::new(compressed)).expect("zstd body"),
        OFFICIAL_SDK_BODY
    );
}

#[test]
fn oauth_request_replaces_client_identity_with_canonical_subscription_profile() {
    let client = Client::builder()
        .no_proxy()
        .build()
        .expect("offline client");
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::USER_AGENT,
        HeaderValue::from_static("codex_exec/9.9.9 (Mac OS 15.0.0; arm64) Apple_Terminal"),
    );
    headers.insert(CODEX_VERSION_HEADER, HeaderValue::from_static("9.9.9"));
    let request = build(
        &client,
        &headers,
        "https://chatgpt.com/backend-api/codex/responses",
        &ResolvedAuth::CodexOAuth {
            token: "oauth-offline-not-real".to_string(),
            account_id: "account-test".to_string(),
        },
        Bytes::from_static(OFFICIAL_SDK_BODY),
    )
    .expect("offline request build");

    assert_eq!(
        request
            .headers()
            .get(http::header::USER_AGENT)
            .and_then(|value| value.to_str().ok()),
        Some(crate::codex_user_agent::canonical_value().as_str())
    );
    assert_eq!(
        request
            .headers()
            .get("originator")
            .and_then(|value| value.to_str().ok()),
        Some(DEFAULT_CODEX_ORIGINATOR)
    );
    assert_eq!(
        request
            .headers()
            .get(CODEX_VERSION_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(CODEX_COMPATIBILITY_VERSION)
    );
}

#[test]
fn websocket_request_emission_matches_codex_header_order_and_deflate_offer() {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (CODEX_VERSION_HEADER, CODEX_COMPATIBILITY_VERSION),
        ("user-agent", "codex_exec/0.149.0"),
        ("originator", "codex_exec"),
        ("x-codex-turn-metadata", r#"{"request_kind":"prewarm"}"#),
        ("x-codex-beta-features", "feature-test"),
        (CODEX_ROUTING_HINT_HEADER, "model=gpt-5.6-sol"),
        ("x-client-request-id", "request-test"),
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
        ("x-codex-window-id", "window-test"),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    let (request, config) = build_websocket(
        &headers,
        "https://example.test/v1/responses",
        &ResolvedAuth::OpenAiApiKey {
            token: "offline-websocket-key-not-real".to_string(),
        },
        1024 * 1024,
    )
    .expect("WebSocket request");
    let (raw, _) = tokio_tungstenite::tungstenite::handshake::client::generate_request(
        request,
        Some(&config.extensions),
    )
    .expect("raw handshake");
    let raw = String::from_utf8(raw).expect("ASCII handshake");
    let names = raw
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':').map(|(name, _)| name))
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        [
            "Host",
            "Connection",
            "Upgrade",
            "Sec-WebSocket-Version",
            "Sec-WebSocket-Key",
            "authorization",
            "user-agent",
            "originator",
            "openai-beta",
            "x-codex-turn-metadata",
            "version",
            "x-codex-beta-features",
            "x-codex-routing-hint",
            "x-client-request-id",
            "session-id",
            "thread-id",
            "x-codex-window-id",
            "sec-websocket-extensions",
        ]
    );
    assert!(
        raw.contains("sec-websocket-extensions: permessage-deflate; client_max_window_bits\r\n")
    );
    assert!(!raw.to_ascii_lowercase().contains("\r\naccept:"));
}

#[test]
fn websocket_subagent_headers_match_codex_conditional_wire_order() {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (CODEX_VERSION_HEADER, CODEX_COMPATIBILITY_VERSION),
        ("user-agent", "codex_exec/0.149.0"),
        ("originator", "codex_exec"),
        ("x-openai-subagent", "review"),
        ("x-codex-beta-features", "feature-test"),
        (CODEX_ROUTING_HINT_HEADER, "model=gpt-5.6-sol"),
        ("x-client-request-id", "request-test"),
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
        ("x-codex-window-id", "window-test"),
        ("x-codex-turn-metadata", r#"{"request_kind":"turn"}"#),
        ("x-codex-parent-thread-id", "parent-test"),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    let (request, config) = build_websocket(
        &headers,
        "https://example.test/v1/responses",
        &ResolvedAuth::OpenAiApiKey {
            token: "offline-websocket-key-not-real".to_string(),
        },
        1024 * 1024,
    )
    .expect("WebSocket request");
    let (raw, _) = tokio_tungstenite::tungstenite::handshake::client::generate_request(
        request,
        Some(&config.extensions),
    )
    .expect("raw handshake");
    let raw = String::from_utf8(raw).expect("ASCII handshake");
    let names = raw
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':').map(|(name, _)| name))
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        [
            "Host",
            "Connection",
            "Upgrade",
            "Sec-WebSocket-Version",
            "Sec-WebSocket-Key",
            "authorization",
            "user-agent",
            "originator",
            "openai-beta",
            "x-openai-subagent",
            "version",
            "x-codex-beta-features",
            "x-codex-routing-hint",
            "x-client-request-id",
            "session-id",
            "thread-id",
            "x-codex-window-id",
            "x-codex-turn-metadata",
            "x-codex-parent-thread-id",
            "sec-websocket-extensions",
        ]
    );
}

#[tokio::test]
async fn http_request_emission_matches_codex_common_header_order() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener");
    let address = listener.local_addr().expect("loopback address");
    let url = format!("http://{address}/v1/responses");
    crate::test_support::assert_loopback_url(&url);

    let mut headers = HeaderMap::new();
    for (name, value) in [
        (CODEX_VERSION_HEADER, CODEX_COMPATIBILITY_VERSION),
        ("x-codex-beta-features", "feature-test"),
        ("x-codex-turn-state", "turn-state-test"),
        ("x-codex-window-id", "window-test"),
        ("x-codex-turn-metadata", r#"{"request_kind":"turn"}"#),
        ("x-codex-parent-thread-id", "parent-test"),
        ("x-openai-subagent", "review"),
        ("x-openai-internal-codex-responses-lite", "true"),
        (CODEX_ROUTING_HINT_HEADER, "model=gpt-5.6-sol"),
        ("x-client-request-id", "request-test"),
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
        ("accept", "text/event-stream"),
        ("content-type", "application/json"),
        ("originator", "codex_exec"),
        ("user-agent", "codex_exec/0.149.0"),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    let client = Client::builder()
        .no_proxy()
        .build()
        .expect("loopback client");
    let request = build(
        &client,
        &headers,
        &url,
        &ResolvedAuth::OpenAiApiKey {
            token: "offline-http-key-not-real".to_string(),
        },
        Bytes::from_static(br#"{"model":"offline"}"#),
    )
    .expect("HTTP request");
    let request_task = tokio::spawn(async move { client.execute(request).await });

    let (mut stream, _) = listener.accept().await.expect("loopback request");
    let mut captured = Vec::new();
    while !captured.windows(4).any(|window| window == b"\r\n\r\n") {
        let mut chunk = [0_u8; 2048];
        let count = stream.read(&mut chunk).await.expect("request bytes");
        assert!(count > 0, "request closed before headers");
        captured.extend_from_slice(&chunk[..count]);
        assert!(captured.len() < 64 * 1024, "request headers too large");
    }
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await
        .expect("loopback response");
    assert_eq!(
        request_task
            .await
            .expect("request task")
            .expect("HTTP response")
            .status(),
        http::StatusCode::OK
    );

    let header_end = captured
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("header terminator");
    let raw_headers = std::str::from_utf8(&captured[..header_end]).expect("ASCII headers");
    let names = raw_headers
        .split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(':').map(|(name, _)| name))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "version",
            "x-codex-beta-features",
            "x-codex-turn-state",
            "x-codex-window-id",
            "x-codex-turn-metadata",
            "x-codex-parent-thread-id",
            "x-openai-subagent",
            "x-openai-internal-codex-responses-lite",
            "x-codex-routing-hint",
            "x-client-request-id",
            "session-id",
            "thread-id",
            "accept",
            "content-type",
            "authorization",
            "originator",
            "user-agent",
            "host",
            "content-length",
        ]
    );
}
