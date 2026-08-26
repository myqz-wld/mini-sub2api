use super::*;

fn offline_client() -> Client {
    Client::builder()
        .no_proxy()
        .build()
        .expect("offline client")
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
        UpstreamProfile::CodexSubscription149,
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
        UpstreamProfile::CodexOpenAi149,
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
        UpstreamProfile::CodexOpenAi149,
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
fn codex_openai_profile_pins_version_and_fills_other_missing_identity_headers() {
    let mut inbound = HeaderMap::new();
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
        UpstreamProfile::CodexOpenAi149,
        Bytes::from_static(br#"{"model":"offline"}"#),
    )
    .expect("Codex OpenAI request");

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
    assert_eq!(
        request
            .headers()
            .get(http::header::USER_AGENT)
            .and_then(|value| value.to_str().ok()),
        Some(crate::codex_user_agent::canonical_value().as_str())
    );

    let (websocket, _) = build_websocket(
        &inbound,
        "https://example.test/v1/responses",
        &ResolvedAuth::OpenAiApiKey {
            token: "offline-profile-key-not-real".to_string(),
        },
        UpstreamProfile::CodexOpenAi149,
        1024,
    )
    .expect("Codex OpenAI WebSocket request");
    assert_eq!(
        websocket
            .headers()
            .get(CODEX_VERSION_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(CODEX_COMPATIBILITY_VERSION)
    );
}
