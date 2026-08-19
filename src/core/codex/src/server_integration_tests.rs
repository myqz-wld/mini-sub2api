use super::*;
use crate::test_support::spawn_loopback;
use crate::test_support::test_jwt;
use axum::Json;
use axum::extract::State as AxumState;
use axum::response::Response as AxumResponse;
use axum::routing::post as axum_post;
use bytes::Bytes;
use futures_util::stream;
use http::HeaderName;
use http::HeaderValue;
use std::convert::Infallible;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

const INTERNAL_TOKEN: &str = "internal-test-token-with-at-least-32-bytes";

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
    assert!(!headers.contains_key("x-forwarded-for"));
    assert!(!headers.contains_key("x-stainless-unreviewed"));
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
            "stream": true
        }))
        .expect("request body"),
    );
    let mut extra_headers = HeaderMap::new();
    for (name, value) in [
        ("originator", "codex_exec"),
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
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

    let response = call_core_with_headers(
        &app_state(vault),
        &metadata.account_ref,
        body,
        extra_headers,
    )
    .await
    .expect("core response");

    assert_eq!(response.status(), StatusCode::OK);
    let captured_body = capture.body.lock().await.clone().expect("captured body");
    let normalized: serde_json::Value =
        serde_json::from_slice(&captured_body).expect("normalized request");
    assert_eq!(normalized["input"][0]["type"], "additional_tools");
    assert_eq!(normalized["input"][0]["tools"], tools);
    assert_eq!(normalized["input"][1]["role"], "developer");
    assert_eq!(normalized["input"][2]["role"], "user");
    assert_eq!(normalized["store"], false);
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
        Some("codex_cli_rs/0.147.0")
    );
    for (name, expected) in [
        ("originator", "codex_exec"),
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
        ("x-openai-internal-codex-responses-lite", "true"),
    ] {
        assert_eq!(
            header_text(&captured_headers, name).as_deref(),
            Some(expected)
        );
    }
    for name in ["openai-organization", "openai-project", "x-stainless-lang"] {
        assert!(!captured_headers.contains_key(name), "header {name}");
    }
}

#[derive(Clone)]
struct OAuthMockState {
    old_access: String,
    new_access: String,
    new_id: String,
    inference_calls: Arc<AtomicUsize>,
    refresh_calls: Arc<AtomicUsize>,
    old_token_barrier: Option<Arc<tokio::sync::Barrier>>,
}

#[tokio::test]
async fn oauth_401_refreshes_and_replays_exactly_once() {
    let account_id = "chatgpt-server-test";
    let mock_state = OAuthMockState {
        old_access: test_jwt(None, 3600),
        new_access: test_jwt(None, 7200),
        new_id: test_jwt(Some(account_id), 7200),
        inference_calls: Arc::new(AtomicUsize::new(0)),
        refresh_calls: Arc::new(AtomicUsize::new(0)),
        old_token_barrier: None,
    };
    let app = Router::new()
        .route(
            "/responses",
            axum_post(
                |AxumState(state): AxumState<OAuthMockState>, headers: HeaderMap| async move {
                    state.inference_calls.fetch_add(1, Ordering::SeqCst);
                    let auth = header_text(&headers, http::header::AUTHORIZATION.as_str());
                    let old_expected = format!("Bearer {}", state.old_access);
                    if auth.as_deref() == Some(old_expected.as_str()) {
                        if let Some(barrier) = state.old_token_barrier {
                            barrier.wait().await;
                        }
                        return (StatusCode::UNAUTHORIZED, "old token");
                    }
                    let expected = format!("Bearer {}", state.new_access);
                    assert_eq!(auth.as_deref(), Some(expected.as_str()));
                    assert_eq!(
                        header_text(&headers, "chatgpt-account-id").as_deref(),
                        Some("chatgpt-server-test")
                    );
                    (StatusCode::OK, "refreshed response")
                },
            ),
        )
        .route(
            "/oauth/token",
            axum_post(|AxumState(state): AxumState<OAuthMockState>| async move {
                state.refresh_calls.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({
                    "id_token": state.new_id,
                    "access_token": state.new_access,
                    "refresh_token": "refresh-new-server-test"
                }))
            }),
        )
        .with_state(mock_state.clone());
    let mock = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: test_jwt(Some(account_id), 3600),
                access_token: mock_state.old_access.clone(),
                refresh_token: "refresh-old-server-test".to_string(),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                issuer: mock.base_url.clone(),
                client_id: "client-server-test".to_string(),
            },
            format!("{}/responses", mock.base_url),
        )
        .await
        .expect("OAuth record");
    let state = app_state(vault);

    let response = call_core(
        &state,
        &metadata.account_ref,
        Bytes::from_static(br#"{"stream":false}"#),
    )
    .await
    .expect("core response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("response body");
    assert_eq!(body, Bytes::from_static(b"refreshed response"));
    assert_eq!(mock_state.inference_calls.load(Ordering::SeqCst), 2);
    assert_eq!(mock_state.refresh_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn concurrent_oauth_401s_share_one_forced_refresh() {
    let account_id = "chatgpt-concurrent-401-test";
    let mock_state = OAuthMockState {
        old_access: test_jwt(None, 3600),
        new_access: test_jwt(None, 7200),
        new_id: test_jwt(Some(account_id), 7200),
        inference_calls: Arc::new(AtomicUsize::new(0)),
        refresh_calls: Arc::new(AtomicUsize::new(0)),
        old_token_barrier: Some(Arc::new(tokio::sync::Barrier::new(2))),
    };
    let app = Router::new()
        .route(
            "/responses",
            axum_post(
                |AxumState(state): AxumState<OAuthMockState>, headers: HeaderMap| async move {
                    state.inference_calls.fetch_add(1, Ordering::SeqCst);
                    let auth = header_text(&headers, http::header::AUTHORIZATION.as_str());
                    let old_expected = format!("Bearer {}", state.old_access);
                    if auth.as_deref() == Some(old_expected.as_str()) {
                        state
                            .old_token_barrier
                            .expect("old-token barrier")
                            .wait()
                            .await;
                        return (StatusCode::UNAUTHORIZED, "old token");
                    }
                    let expected = format!("Bearer {}", state.new_access);
                    assert_eq!(auth.as_deref(), Some(expected.as_str()));
                    (StatusCode::OK, "refreshed response")
                },
            ),
        )
        .route(
            "/oauth/token",
            axum_post(|AxumState(state): AxumState<OAuthMockState>| async move {
                state.refresh_calls.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({
                    "id_token": state.new_id,
                    "access_token": state.new_access,
                    "refresh_token": "refresh-new-concurrent-test"
                }))
            }),
        )
        .with_state(mock_state.clone());
    let mock = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: test_jwt(Some(account_id), 3600),
                access_token: mock_state.old_access.clone(),
                refresh_token: "refresh-old-concurrent-test".to_string(),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                issuer: mock.base_url.clone(),
                client_id: "client-concurrent-test".to_string(),
            },
            format!("{}/responses", mock.base_url),
        )
        .await
        .expect("OAuth record");
    let state = app_state(vault);
    let body = Bytes::from_static(br#"{"stream":false}"#);
    let (first, second) = tokio::join!(
        call_core(&state, &metadata.account_ref, body.clone()),
        call_core(&state, &metadata.account_ref, body),
    );
    assert_eq!(first.expect("first response").status(), StatusCode::OK);
    assert_eq!(second.expect("second response").status(), StatusCode::OK);
    assert_eq!(mock_state.inference_calls.load(Ordering::SeqCst), 4);
    assert_eq!(mock_state.refresh_calls.load(Ordering::SeqCst), 1);
}

async fn api_key_state(base_url: &str) -> (AppState, String, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-api-key-test".to_string(),
            format!("{base_url}/responses"),
        )
        .await
        .expect("API key record");
    (app_state(vault), metadata.account_ref, temp)
}

fn app_state(vault: Vault) -> AppState {
    AppState {
        vault,
        client: Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client"),
        direct_client: Client::builder().no_proxy().build().expect("direct client"),
        internal_token_hash: Sha256::digest(INTERNAL_TOKEN.as_bytes()).into(),
        account_locks: Arc::new(Mutex::new(HashMap::new())),
    }
}

async fn call_core(
    state: &AppState,
    account_ref: &str,
    body: Bytes,
) -> std::result::Result<Response<Body>, CoreFailure> {
    call_core_with_headers(state, account_ref, body, HeaderMap::new()).await
}

async fn call_core_with_headers(
    state: &AppState,
    account_ref: &str,
    body: Bytes,
    extra_headers: HeaderMap,
) -> std::result::Result<Response<Body>, CoreFailure> {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer internal-test-token-with-at-least-32-bytes"),
    );
    headers.insert(VERSION_HEADER, HeaderValue::from_static(VERSION));
    headers.insert(
        ACCOUNT_REF_HEADER,
        HeaderValue::from_str(account_ref).expect("account ref"),
    );
    headers.insert(REQUEST_ID_HEADER, HeaderValue::from_static("req_test"));
    headers.insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert("x-codex-turn-state", HeaderValue::from_static("turn-test"));
    headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.10"));
    headers.extend(extra_headers);
    let request = Request::builder().body(Body::from(body)).expect("request");
    responses_inner(
        "127.0.0.1:43210".parse().expect("peer"),
        state,
        headers,
        request,
    )
    .await
}
