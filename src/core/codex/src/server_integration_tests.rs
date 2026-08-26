use super::*;
use crate::test_support::spawn_loopback;
use crate::test_support::test_jwt;
use axum::extract::State as AxumState;
use axum::response::Response as AxumResponse;
use axum::routing::post as axum_post;
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream;
use http::HeaderName;
use http::HeaderValue;
use http_body_util::BodyExt;
use mini_sub2api_protocol_v1::CORE_TTFB_HEADER;
use mini_sub2api_protocol_v1::DELIVERY_STATE_TRAILER;
use mini_sub2api_protocol_v1::FAILURE_PHASE_TRAILER;
use mini_sub2api_protocol_v1::RETRY_ADVICE_TRAILER;
use std::convert::Infallible;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::integration_support::api_key_state;
use super::integration_support::app_state;
use super::integration_support::call_core_with_headers;

#[derive(Clone, Default)]
struct ApiCapture {
    headers: Arc<Mutex<Option<HeaderMap>>>,
    body: Arc<Mutex<Option<Bytes>>>,
    calls: Arc<AtomicUsize>,
}

#[tokio::test]
async fn api_key_route_preserves_stream_and_replaces_sensitive_headers() {
    let capture = ApiCapture::default();
    let app = Router::new()
        .route(
            "/responses",
            axum_post(
                |AxumState(capture): AxumState<ApiCapture>,
                 headers: HeaderMap,
                 body: Bytes| async move {
                    capture.calls.fetch_add(1, Ordering::SeqCst);
                    *capture.headers.lock().await = Some(headers);
                    *capture.body.lock().await = Some(body);
                    let chunks = stream::unfold(0_u8, |step| async move {
                        match step {
                            0 => Some((
                                Ok::<_, Infallible>(Bytes::from_static(b"data: first\n\n")),
                                1,
                            )),
                            1 => {
                                tokio::time::sleep(Duration::from_millis(30)).await;
                                Some((
                                    Ok(Bytes::from_static(b"data: completed\n\n")),
                                    2,
                                ))
                            }
                            _ => None,
                        }
                    });
                    AxumResponse::builder()
                        .status(StatusCode::OK)
                        .header(http::header::SET_COOKIE, "must-not-cross=1")
                        .header(CORE_TTFB_HEADER, "forged-upstream-value")
                        .header(http::header::CONNECTION, "x-hop-test")
                        .header("x-hop-test", "must-not-cross")
                        .body(Body::from_stream(chunks))
                        .expect("mock response")
                },
            ),
        )
        .with_state(capture.clone());
    let mock = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&mock.base_url).await;
    let inbound_body = Bytes::from_static(br#"{"model":"test","stream":true}"#);
    let mut extra_headers = HeaderMap::new();
    for (name, value) in [
        ("accept", "text/event-stream"),
        ("user-agent", "OpenAI/Go 3.52.0"),
        ("openai-organization", "org-test"),
        ("openai-project", "proj-test"),
        ("x-stainless-arch", "arm64"),
        ("x-stainless-lang", "go"),
        ("x-stainless-package-version", "3.52.0"),
        ("x-stainless-retry-count", "0"),
        ("x-stainless-runtime", "go"),
        ("x-stainless-runtime-version", "go1.26.0"),
        ("x-stainless-unreviewed", "must-not-cross"),
    ] {
        extra_headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }

    let response =
        call_core_with_headers(&state, &account_ref, inbound_body.clone(), extra_headers)
            .await
            .expect("core response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    assert!(response.headers().contains_key(CORE_TTFB_HEADER));
    assert_eq!(
        response.headers().get_all(CORE_TTFB_HEADER).iter().count(),
        1
    );
    assert!(!response.headers().contains_key(http::header::SET_COOKIE));
    assert!(!response.headers().contains_key("x-hop-test"));
    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_millis(200), stream.next())
        .await
        .expect("first chunk timeout")
        .expect("first chunk")
        .expect("first chunk data");
    assert_eq!(first, Bytes::from_static(b"data: first\n\n"));
    let second = stream.next().await.expect("second chunk").expect("data");
    assert_eq!(second, Bytes::from_static(b"data: completed\n\n"));

    assert_eq!(capture.calls.load(Ordering::SeqCst), 1);
    assert_eq!(capture.body.lock().await.as_ref(), Some(&inbound_body));
    let headers = capture.headers.lock().await.clone().expect("headers");
    assert_eq!(
        header_text(&headers, http::header::AUTHORIZATION.as_str()).as_deref(),
        Some("Bearer upstream-api-key-test")
    );
    assert_eq!(
        header_text(&headers, "x-codex-turn-state").as_deref(),
        Some("turn-test")
    );
    for (name, expected) in [
        ("accept", "text/event-stream"),
        ("user-agent", "OpenAI/Go 3.52.0"),
        ("openai-organization", "org-test"),
        ("openai-project", "proj-test"),
        ("x-stainless-arch", "arm64"),
        ("x-stainless-lang", "go"),
        ("x-stainless-package-version", "3.52.0"),
        ("x-stainless-retry-count", "0"),
        ("x-stainless-runtime", "go"),
        ("x-stainless-runtime-version", "go1.26.0"),
    ] {
        assert_eq!(header_text(&headers, name).as_deref(), Some(expected));
    }
    assert!(!headers.contains_key(ACCOUNT_REF_HEADER));
    assert!(!headers.contains_key(PSEUDONYM_SCOPE_HEADER));
    assert!(!headers.contains_key("x-forwarded-for"));
    assert!(!headers.contains_key("x-stainless-unreviewed"));
}

#[tokio::test]
async fn upstream_stream_failure_becomes_delivery_trailers() {
    let app = Router::new().route(
        "/responses",
        axum_post(|| async {
            let chunks = stream::unfold(0_u8, |step| async move {
                match step {
                    0 => Some((
                        Ok::<_, std::io::Error>(Bytes::from_static(b"data: first\n\n")),
                        1,
                    )),
                    1 => {
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        Some((Err(std::io::Error::other("simulated upstream reset")), 2))
                    }
                    _ => None,
                }
            });
            AxumResponse::builder()
                .status(StatusCode::OK)
                .header(http::header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(chunks))
                .expect("mock response")
        }),
    );
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let response = call_core_with_headers(
        &state,
        &account_ref,
        Bytes::from_static(br#"{"model":"test","stream":true}"#),
        HeaderMap::new(),
    )
    .await
    .expect("core response");
    assert!(response.headers().contains_key(http::header::TRAILER));

    let mut body = response.into_body();
    let data = body
        .frame()
        .await
        .expect("data frame")
        .expect("valid data frame")
        .into_data()
        .expect("data");
    assert_eq!(data, Bytes::from_static(b"data: first\n\n"));
    let trailers = body
        .frame()
        .await
        .expect("trailer frame")
        .expect("valid trailer frame")
        .into_trailers()
        .expect("trailers");
    assert_eq!(trailers[FAILURE_PHASE_TRAILER], "upstream_stream");
    assert_eq!(trailers[DELIVERY_STATE_TRAILER], "delivered");
    assert_eq!(trailers[RETRY_ADVICE_TRAILER], "never");
}

#[tokio::test]
async fn subscription_route_normalizes_plain_request_and_preserves_client_tools() {
    let capture = ApiCapture::default();
    let app = Router::new()
        .route(
            "/responses",
            axum_post(
                |AxumState(capture): AxumState<ApiCapture>,
                 headers: HeaderMap,
                 body: Bytes| async move {
                    capture.calls.fetch_add(1, Ordering::SeqCst);
                    *capture.headers.lock().await = Some(headers);
                    *capture.body.lock().await = Some(body);
                    (StatusCode::OK, "normalized")
                },
            ),
        )
        .with_state(capture.clone());
    let mock = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let account_id = "chatgpt-normalizer-test";
    let access_token = test_jwt(None, 3600);
    let metadata = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: test_jwt(Some(account_id), 3600),
                access_token: access_token.clone(),
                refresh_token: "refresh-normalizer-test".to_string(),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                issuer: mock.base_url.clone(),
                client_id: "client-normalizer-test".to_string(),
            },
            format!("{}/responses", mock.base_url),
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let tools = serde_json::json!([
        {"type":"function","name":"lookup","description":"Lookup","parameters":{"type":"object"}},
        {"type":"web_search_preview"}
    ]);
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "gpt-5.6-sol",
            "instructions": "Be concise",
            "input": "hello",
            "tools": tools,
            "stream": true,
            "max_output_tokens": 32768,
            "client_metadata": {
                "x-codex-installation-id": "body-device-conflict",
                "x-codex-turn-metadata": serde_json::json!({
                    "installation_id": "body-turn-device-conflict",
                    "session_id": "body-session-kept",
                    "thread_id": "body-thread-kept",
                    "turn_id": "body-turn-kept",
                    "window_id": "body-window-kept",
                    "future": {"kept": true}
                }).to_string()
            }
        }))
        .expect("request body"),
    );
    let mut extra_headers = HeaderMap::new();
    for (name, value) in [
        ("originator", "codex_exec"),
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
        ("x-codex-installation-id", "header-device-conflict"),
        (
            "x-codex-turn-metadata",
            r#"{"installation_id":"header-turn-device-conflict","session_id":"header-session-kept","future":1}"#,
        ),
        ("x-openai-internal-codex-responses-lite", "true"),
        ("openai-organization", "must-not-cross"),
        ("openai-project", "must-not-cross"),
        ("x-stainless-lang", "must-not-cross"),
    ] {
        extra_headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_str(value).expect("header value"),
        );
    }

    let state = app_state(vault);
    let expected_device =
        crate::request_pseudonym::RequestPseudonymizer::converged_installation_id(account_id);
    let response = call_core_with_headers(&state, &metadata.account_ref, body, extra_headers)
        .await
        .expect("core response");

    assert_eq!(response.status(), StatusCode::OK);
    let captured_body = capture.body.lock().await.clone().expect("captured body");
    let captured_body = zstd::stream::decode_all(std::io::Cursor::new(captured_body.as_ref()))
        .expect("decompress normalized request");
    let normalized: serde_json::Value =
        serde_json::from_slice(&captured_body).expect("normalized request");
    assert_eq!(normalized["input"][0]["type"], "additional_tools");
    assert_eq!(normalized["input"][0]["tools"][0]["type"], "namespace");
    assert_eq!(normalized["input"][0]["tools"][0]["name"], "functions");
    assert_eq!(
        normalized["input"][0]["tools"][0]["tools"][0]["name"],
        "lookup"
    );
    assert_eq!(normalized["input"][0]["tools"][1], tools[1]);
    assert_eq!(normalized["input"][1]["role"], "developer");
    assert_eq!(normalized["input"][2]["role"], "user");
    assert_eq!(normalized["store"], false);
    assert!(normalized.get("max_output_tokens").is_none());
    assert!(
        normalized["client_metadata"]["x-codex-installation-id"].as_str()
            == Some(expected_device.as_str())
    );
    let body_turn: serde_json::Value = serde_json::from_str(
        normalized["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("body turn metadata"),
    )
    .expect("body turn JSON");
    assert!(body_turn["installation_id"].as_str() == Some(expected_device.as_str()));
    for (name, raw) in [
        ("session_id", "body-session-kept"),
        ("thread_id", "body-thread-kept"),
        ("turn_id", "body-turn-kept"),
        ("window_id", "body-window-kept"),
    ] {
        let pseudonym = body_turn[name].as_str().expect("pseudonym");
        assert_ne!(pseudonym, raw);
        assert_eq!(
            uuid::Uuid::parse_str(pseudonym)
                .expect("pseudonym UUID")
                .get_version_num(),
            8
        );
    }
    assert!(body_turn.get("future").is_none());
    let captured_headers = capture.headers.lock().await.clone().expect("headers");
    assert_eq!(
        header_text(&captured_headers, http::header::AUTHORIZATION.as_str()).as_deref(),
        Some(format!("Bearer {access_token}").as_str())
    );
    assert_eq!(
        header_text(&captured_headers, "chatgpt-account-id").as_deref(),
        Some(account_id)
    );
    assert_eq!(
        header_text(&captured_headers, http::header::USER_AGENT.as_str()).as_deref(),
        Some(crate::codex_user_agent::canonical_value().as_str())
    );
    assert!(!captured_headers.contains_key("x-codex-installation-id"));
    assert_eq!(
        header_text(&captured_headers, "content-encoding").as_deref(),
        Some("zstd")
    );
    assert_eq!(
        header_text(&captured_headers, "x-codex-routing-hint").as_deref(),
        Some("model=gpt-5.6-sol")
    );
    let header_turn: serde_json::Value = serde_json::from_str(
        header_text(&captured_headers, "x-codex-turn-metadata")
            .as_deref()
            .expect("header turn metadata"),
    )
    .expect("header turn JSON");
    assert!(header_turn["installation_id"].as_str() == Some(expected_device.as_str()));
    assert_ne!(header_turn["session_id"], "header-session-kept");
    assert!(header_turn.get("future").is_none());
    for (name, expected) in [
        ("originator", "codex_cli_rs"),
        ("x-openai-internal-codex-responses-lite", "true"),
    ] {
        assert_eq!(
            header_text(&captured_headers, name).as_deref(),
            Some(expected)
        );
    }
    for (name, raw) in [("session-id", "session-test"), ("thread-id", "thread-test")] {
        let pseudonym = header_text(&captured_headers, name).expect("identity header");
        assert_ne!(pseudonym, raw);
        assert_eq!(
            uuid::Uuid::parse_str(&pseudonym)
                .expect("pseudonym UUID")
                .get_version_num(),
            8
        );
    }
    for name in ["openai-organization", "openai-project", "x-stainless-lang"] {
        assert!(!captured_headers.contains_key(name), "header {name}");
    }
    assert!(!captured_headers.contains_key(PSEUDONYM_SCOPE_HEADER));
}
