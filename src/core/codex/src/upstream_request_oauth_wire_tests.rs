use super::*;
use std::collections::BTreeSet;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

#[test]
fn oauth_websocket_headers_match_codex_0149_raw_order() {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (CODEX_VERSION_HEADER, CODEX_COMPATIBILITY_VERSION),
        (
            "user-agent",
            "codex_cli_rs/0.149.0 (Mac OS test; arm64) dumb",
        ),
        ("originator", "codex_cli_rs"),
        ("x-codex-beta-features", "remote_compaction_v2"),
        (CODEX_ROUTING_HINT_HEADER, "model=gpt-5.4"),
        ("x-client-request-id", "thread-test"),
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
        ("x-codex-window-id", "thread-test:0"),
        ("x-codex-turn-metadata", r#"{"request_kind":"prewarm"}"#),
        ("x-codex-parent-thread-id", "parent-test"),
        ("x-openai-subagent", "review"),
        ("x-codex-turn-state", "frame-only"),
        ("x-codex-installation-id", "body-only"),
        ("x-openai-internal-codex-responses-lite", "frame-only"),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    let (request, config) = build_websocket(
        &headers,
        "https://example.test/v1/responses",
        &ResolvedAuth::CodexOAuth {
            token: "oauth-websocket-not-real".to_string(),
            account_id: "account-test".to_string(),
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
            "chatgpt-account-id",
            "authorization",
            "user-agent",
            "openai-beta",
            "x-codex-routing-hint",
            "version",
            "x-codex-beta-features",
            "originator",
            "x-client-request-id",
            "session-id",
            "thread-id",
            "x-codex-window-id",
            "x-codex-turn-metadata",
            "x-codex-parent-thread-id",
            "x-openai-subagent",
            "sec-websocket-extensions",
        ]
    );
    for forbidden in [
        "x-codex-turn-state",
        "x-codex-installation-id",
        "x-openai-internal-codex-responses-lite",
    ] {
        assert!(
            !raw.to_ascii_lowercase()
                .contains(&format!("\r\n{forbidden}:"))
        );
    }
}

#[test]
fn oauth_websocket_timing_headers_match_codex_0149_conditional_order() {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (CODEX_VERSION_HEADER, CODEX_COMPATIBILITY_VERSION),
        (
            "user-agent",
            "codex_cli_rs/0.149.0 (Mac OS test; arm64) dumb",
        ),
        ("originator", "codex_cli_rs"),
        ("x-codex-beta-features", "remote_compaction_v2"),
        (CODEX_ROUTING_HINT_HEADER, "model=gpt-5.4"),
        ("x-client-request-id", "thread-test"),
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
        ("x-codex-window-id", "thread-test:0"),
        ("x-codex-turn-metadata", r#"{"request_kind":"prewarm"}"#),
        ("x-codex-parent-thread-id", "parent-test"),
        ("x-openai-subagent", "review"),
        ("x-openai-internal-codex-residency", "us"),
        ("x-responsesapi-include-timing-metrics", "true"),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    let (request, config) = build_websocket(
        &headers,
        "https://example.test/v1/responses",
        &ResolvedAuth::CodexOAuth {
            token: "oauth-websocket-not-real".to_string(),
            account_id: "account-test".to_string(),
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
            "chatgpt-account-id",
            "authorization",
            "user-agent",
            "x-responsesapi-include-timing-metrics",
            "openai-beta",
            "version",
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
            "x-codex-routing-hint",
            "sec-websocket-extensions",
        ]
    );
}

#[tokio::test]
async fn oauth_http_headers_and_zstd_body_match_codex_0149_wire_shape() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener");
    let address = listener.local_addr().expect("loopback address");
    let url = format!("http://{address}/v1/responses");
    crate::test_support::assert_loopback_url(&url);

    let mut headers = HeaderMap::new();
    for (name, value) in [
        (CODEX_VERSION_HEADER, CODEX_COMPATIBILITY_VERSION),
        ("x-openai-internal-codex-residency", "us"),
        ("x-codex-beta-features", "remote_compaction_v2"),
        ("x-codex-window-id", "thread-test:0"),
        ("x-codex-turn-metadata", r#"{"request_kind":"turn"}"#),
        (CODEX_ROUTING_HINT_HEADER, "model=gpt-5.4,tier=priority"),
        ("x-client-request-id", "thread-test"),
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
        ("accept", "application/json"),
        ("content-type", "text/plain"),
        ("openai-beta", "websocket-only"),
        ("originator", "codex_cli_rs"),
        (
            "user-agent",
            "codex_cli_rs/0.149.0 (Mac OS test; arm64) dumb",
        ),
        ("x-codex-installation-id", "body-only"),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    let body = Bytes::from_static(br#"{"model":"gpt-5.4","input":[],"stream":true}"#);
    let client = Client::builder()
        .no_proxy()
        .build()
        .expect("loopback client");
    let request = build(
        &client,
        &headers,
        &url,
        &ResolvedAuth::CodexOAuth {
            token: "oauth-http-not-real".to_string(),
            account_id: "account-test".to_string(),
        },
        body.clone(),
    )
    .expect("HTTP request");
    let request_task = tokio::spawn(async move { client.execute(request).await });

    let (mut stream, _) = listener.accept().await.expect("loopback request");
    let mut captured = Vec::new();
    let (header_end, content_length) = loop {
        let mut chunk = [0_u8; 2048];
        let count = stream.read(&mut chunk).await.expect("request bytes");
        assert!(count > 0, "request closed before headers");
        captured.extend_from_slice(&chunk[..count]);
        assert!(captured.len() < 64 * 1024, "request headers too large");
        if let Some(header_end) = captured.windows(4).position(|window| window == b"\r\n\r\n") {
            let raw = std::str::from_utf8(&captured[..header_end]).expect("ASCII headers");
            let length = raw
                .split("\r\n")
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("content length"))
                })
                .expect("content-length header");
            break (header_end, length);
        }
    };
    let body_start = header_end + 4;
    while captured.len() < body_start + content_length {
        let mut chunk = [0_u8; 2048];
        let count = stream.read(&mut chunk).await.expect("body bytes");
        assert!(count > 0, "request closed before body");
        captured.extend_from_slice(&chunk[..count]);
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
            "x-openai-internal-codex-residency",
            "x-codex-beta-features",
            "x-codex-window-id",
            "x-codex-turn-metadata",
            "x-codex-routing-hint",
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
            "host",
            "content-length",
        ]
    );
    for forbidden in [
        "x-codex-turn-state",
        "x-codex-installation-id",
        "x-openai-internal-codex-responses-lite",
    ] {
        assert!(
            !raw_headers
                .to_ascii_lowercase()
                .contains(&format!("\r\n{forbidden}:"))
        );
    }
    assert!(raw_headers.contains("\r\naccept: text/event-stream\r\n"));
    assert!(raw_headers.contains("\r\ncontent-type: application/json\r\n"));
    assert!(!raw_headers.contains("\r\nopenai-beta:"));
    let decoded = zstd::stream::decode_all(&captured[body_start..body_start + content_length])
        .expect("zstd body");
    assert_eq!(decoded, body.as_ref());
}

#[test]
fn wire_order_tables_cover_every_reviewed_header_without_duplicates() {
    let http = HTTP_HEADER_ORDER.iter().copied().collect::<BTreeSet<_>>();
    let websocket = WEBSOCKET_WIRE_HEADER_ORDER
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let websocket_subagent = WEBSOCKET_SUBAGENT_WIRE_HEADER_ORDER
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let oauth_websocket = OAUTH_WEBSOCKET_WIRE_HEADER_ORDER
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let oauth_websocket_timing = OAUTH_WEBSOCKET_TIMING_WIRE_HEADER_ORDER
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(http.len(), HTTP_HEADER_ORDER.len());
    assert_eq!(websocket.len(), WEBSOCKET_WIRE_HEADER_ORDER.len());
    assert_eq!(
        websocket_subagent.len(),
        WEBSOCKET_SUBAGENT_WIRE_HEADER_ORDER.len()
    );
    assert_eq!(
        oauth_websocket.len(),
        OAUTH_WEBSOCKET_WIRE_HEADER_ORDER.len()
    );
    assert_eq!(
        oauth_websocket_timing.len(),
        OAUTH_WEBSOCKET_TIMING_WIRE_HEADER_ORDER.len()
    );

    for name in COMMON_ALLOWED.iter().chain(OPENAI_API_KEY_ALLOWED) {
        assert!(http.contains(name), "HTTP order omitted {name}");
        if !["accept", "content-encoding", "content-type"].contains(name) {
            assert!(websocket.contains(name), "WebSocket order omitted {name}");
            assert!(
                websocket_subagent.contains(name),
                "WebSocket subagent order omitted {name}"
            );
        }
    }
    for name in ["authorization", "chatgpt-account-id"] {
        assert!(http.contains(name));
        assert!(websocket.contains(name));
        assert!(websocket_subagent.contains(name));
        assert!(oauth_websocket.contains(name));
    }
}
