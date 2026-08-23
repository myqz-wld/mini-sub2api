use super::*;
use crate::vault::DEFAULT_OPENAI_RESPONSES_URL;
use pretty_assertions::assert_eq;

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
        Some("mini_sub2api")
    );
    assert_eq!(
        request
            .headers()
            .get(http::header::USER_AGENT)
            .and_then(|value| value.to_str().ok()),
        Some("codex_cli_rs/0.149.0")
    );
    for name in OPENAI_API_KEY_ALLOWED {
        assert!(!request.headers().contains_key(*name), "header {name}");
    }
}

#[test]
fn oauth_request_pins_existing_codex_user_agent_and_preserves_suffix() {
    let client = Client::builder()
        .no_proxy()
        .build()
        .expect("offline client");
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::USER_AGENT,
        HeaderValue::from_static("codex_exec/9.9.9 (Mac OS 15.0.0; arm64) Apple_Terminal"),
    );
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
        Some("codex_exec/0.149.0 (Mac OS 15.0.0; arm64) Apple_Terminal")
    );
}
