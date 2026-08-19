use super::*;
use crate::test_support::spawn_loopback;
use crate::test_support::test_jwt;
use axum::Json;
use axum::extract::State as AxumState;
use axum::response::Response as AxumResponse;
use axum::routing::post as axum_post;
use bytes::Bytes;
use futures_util::stream;
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
                        .header(http::header::CONTENT_TYPE, "text/event-stream")
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

    let response = call_core(&state, &account_ref, inbound_body.clone())
        .await
        .expect("core response");
    assert_eq!(response.status(), StatusCode::OK);
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
    assert!(!headers.contains_key(ACCOUNT_REF_HEADER));
    assert!(!headers.contains_key("x-forwarded-for"));
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
    let request = Request::builder().body(Body::from(body)).expect("request");
    responses_inner(
        "127.0.0.1:43210".parse().expect("peer"),
        state,
        headers,
        request,
    )
    .await
}
