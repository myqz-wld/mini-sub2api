use super::*;

fn offline_client() -> Client {
    Client::builder()
        .no_proxy()
        .build()
        .expect("offline client")
}

#[test]
fn guardian_headers_survive_http_and_websocket_without_crossing_response_privacy() {
    let mut headers = HeaderMap::new();
    headers.insert("x-codex-guardian", HeaderValue::from_static("reviewer"));
    headers.insert(
        http::header::COOKIE,
        HeaderValue::from_static("synthetic-private"),
    );
    let auth = ResolvedAuth::CodexOAuth {
        token: "synthetic-token".into(),
        account_id: "synthetic-account".into(),
    };
    let http = build(
        &offline_client(),
        &headers,
        "http://127.0.0.1:1/responses",
        &auth,
        UpstreamProfile::CodexSubscription1580,
        Bytes::from_static(b"{}"),
    )
    .unwrap();
    let (ws, _) = build_websocket(
        &headers,
        "http://127.0.0.1:1/responses",
        &auth,
        UpstreamProfile::CodexSubscription1580,
        1024,
    )
    .unwrap();
    for emitted in [http.headers(), ws.headers()] {
        assert_eq!(emitted["x-codex-guardian"], "reviewer");
        assert!(!emitted.contains_key(http::header::COOKIE));
    }
}

#[test]
fn guardian_reviewer_uses_native_01580_late_header_merge_order() {
    // Pinned ModelClient reviewer builder + endpoint/default merge, followed by
    // tungstenite handshake serialization. Ordinary and classifier controls live
    // in oauth_wire_tests and classifier_tests.
    let http_order = [
        "version",
        "x-codex-beta-features",
        "x-codex-window-id",
        "x-codex-turn-metadata",
        "x-codex-parent-thread-id",
        "x-openai-subagent",
        "x-openai-internal-codex-responses-lite",
        "x-codex-guardian",
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
    ];
    let mut headers = HeaderMap::new();
    for name in http_order {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static("synthetic"),
        );
    }
    headers.insert("x-codex-guardian", HeaderValue::from_static("reviewer"));
    headers.remove("content-encoding");
    let auth = ResolvedAuth::CodexOAuth {
        token: "synthetic".into(),
        account_id: "synthetic".into(),
    };
    let request = build(
        &offline_client(),
        &headers,
        "http://127.0.0.1:1/responses",
        &auth,
        UpstreamProfile::CodexSubscription1580,
        Bytes::from_static(b"{}"),
    )
    .unwrap();
    assert_eq!(
        request
            .headers()
            .keys()
            .map(|n| n.as_str())
            .collect::<Vec<_>>(),
        http_order
    );
    let (request, config) = build_websocket(
        &headers,
        "http://127.0.0.1:1/responses",
        &auth,
        UpstreamProfile::CodexSubscription1580,
        4096,
    )
    .unwrap();
    let (bytes, _) = tokio_tungstenite::tungstenite::handshake::client::generate_request(
        request,
        Some(&config.extensions),
    )
    .unwrap();
    let raw = std::str::from_utf8(&bytes).unwrap();
    let names = raw
        .lines()
        .skip(1)
        .filter_map(|line| {
            line.split_once(':')
                .map(|(name, _)| name.to_ascii_lowercase())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "host",
            "connection",
            "upgrade",
            "sec-websocket-version",
            "sec-websocket-key",
            "chatgpt-account-id",
            "authorization",
            "user-agent",
            "originator",
            "x-codex-guardian",
            "version",
            "x-codex-beta-features",
            "x-client-request-id",
            "session-id",
            "thread-id",
            "x-codex-window-id",
            "x-codex-turn-metadata",
            "x-codex-parent-thread-id",
            "x-openai-subagent",
            "openai-beta",
            "sec-websocket-extensions",
        ]
    );
}

#[test]
fn originator_profile_cannot_promote_an_api_key_to_subscription_auth() {
    let auth = ResolvedAuth::OpenAiApiKey {
        token: "offline-profile-key-not-real".to_string(),
    };
    let result = build(
        &offline_client(),
        &HeaderMap::new(),
        "https://example.test/v1/responses",
        &auth,
        UpstreamProfile::CodexSubscription1580,
        Bytes::from_static(br#"{"model":"offline"}"#),
    );

    assert!(matches!(result, Err(CoreFailure::Internal)));
}

#[test]
fn api_key_profile_cannot_be_used_with_subscription_auth() {
    let auth = ResolvedAuth::CodexOAuth {
        token: "offline-profile-token-not-real".to_string(),
        account_id: "offline-account".to_string(),
    };
    let result = build_websocket(
        &HeaderMap::new(),
        "https://example.test/v1/responses",
        &auth,
        UpstreamProfile::ApiKeyPassthrough,
        1024,
    );

    assert!(matches!(result, Err(CoreFailure::Internal)));
}

#[test]
fn codex_openai_profile_keeps_http_body_uncompressed() {
    let body = Bytes::from_static(br#" {"model":"offline","future":true} "#);
    let request = build(
        &offline_client(),
        &HeaderMap::new(),
        "https://example.test/v1/responses",
        &ResolvedAuth::OpenAiApiKey {
            token: "offline-profile-key-not-real".to_string(),
        },
        UpstreamProfile::ApiKeyPassthrough,
        body.clone(),
    )
    .expect("Codex OpenAI request");

    assert!(
        !request
            .headers()
            .contains_key(http::header::CONTENT_ENCODING)
    );
    assert_eq!(
        request.body().and_then(reqwest::Body::as_bytes),
        Some(body.as_ref())
    );
}

#[test]
fn api_key_profile_preserves_the_complete_client_identity() {
    let mut inbound = HeaderMap::new();
    inbound.insert(
        http::header::USER_AGENT,
        HeaderValue::from_static("codex_exec/9.9.9 (Mac OS 15.0.0; arm64) Apple_Terminal"),
    );
    inbound.insert("originator", HeaderValue::from_static("codex_exec"));
    inbound.insert(
        CODEX_VERSION_HEADER,
        HeaderValue::from_static("caller-version-must-not-survive"),
    );
    let request = build(
        &offline_client(),
        &inbound,
        "https://example.test/v1/responses",
        &ResolvedAuth::OpenAiApiKey {
            token: "offline-profile-key-not-real".to_string(),
        },
        UpstreamProfile::ApiKeyPassthrough,
        Bytes::from_static(br#"{"model":"offline"}"#),
    )
    .expect("Codex OpenAI request");

    assert_eq!(
        request
            .headers()
            .get("originator")
            .and_then(|value| value.to_str().ok()),
        Some("codex_exec")
    );
    assert_eq!(
        request
            .headers()
            .get(CODEX_VERSION_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("caller-version-must-not-survive")
    );
    assert_eq!(
        request
            .headers()
            .get(http::header::USER_AGENT)
            .and_then(|value| value.to_str().ok()),
        Some("codex_exec/9.9.9 (Mac OS 15.0.0; arm64) Apple_Terminal")
    );

    let (websocket, _) = build_websocket(
        &inbound,
        "https://example.test/v1/responses",
        &ResolvedAuth::OpenAiApiKey {
            token: "offline-profile-key-not-real".to_string(),
        },
        UpstreamProfile::ApiKeyPassthrough,
        1024,
    )
    .expect("Codex OpenAI WebSocket request");
    assert_eq!(
        websocket
            .headers()
            .get(http::header::USER_AGENT)
            .and_then(|value| value.to_str().ok()),
        Some("codex_exec/9.9.9 (Mac OS 15.0.0; arm64) Apple_Terminal")
    );
    assert_eq!(
        websocket
            .headers()
            .get("originator")
            .and_then(|value| value.to_str().ok()),
        Some("codex_exec")
    );
    assert_eq!(
        websocket
            .headers()
            .get(CODEX_VERSION_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("caller-version-must-not-survive")
    );
}
