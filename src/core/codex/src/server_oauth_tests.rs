use super::*;
use crate::test_support::spawn_loopback;
use crate::test_support::test_jwt;
use axum::Json;
use axum::extract::State as AxumState;
use axum::routing::post as axum_post;
use bytes::Bytes;
use http::HeaderValue;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use super::integration_support::app_state;
use super::integration_support::call_core;
use super::integration_support::call_core_with_headers;

#[derive(Clone)]
struct OAuthMockState {
    old_access: String,
    new_access: String,
    new_id: String,
    inference_calls: Arc<AtomicUsize>,
    refresh_calls: Arc<AtomicUsize>,
    old_token_barrier: Option<Arc<tokio::sync::Barrier>>,
    fingerprint_headers: Arc<Mutex<Vec<String>>>,
    inference_call_ids: Arc<Mutex<Vec<Option<String>>>>,
    bodies: Arc<Mutex<Vec<Bytes>>>,
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
        fingerprint_headers: Arc::new(Mutex::new(Vec::new())),
        inference_call_ids: Arc::new(Mutex::new(Vec::new())),
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let app =
        Router::new()
            .route(
                "/responses",
                axum_post(
                    |AxumState(state): AxumState<OAuthMockState>,
                     headers: HeaderMap,
                     body: Bytes| async move {
                        state.inference_calls.fetch_add(1, Ordering::SeqCst);
                        state.bodies.lock().await.push(body);
                        assert_eq!(
                            header_text(&headers, crate::upstream_request::CODEX_VERSION_HEADER)
                                .as_deref(),
                            Some(crate::upstream_request::CODEX_COMPATIBILITY_VERSION)
                        );
                        if let Some(device) = header_text(&headers, "x-codex-installation-id") {
                            state.fingerprint_headers.lock().await.push(device);
                        }
                        state
                            .inference_call_ids
                            .lock()
                            .await
                            .push(header_text(&headers, "x-codex-inference-call-id"));
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
                        (
                            StatusCode::OK,
                            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_refreshed\",\"object\":\"response\"}}\n\n",
                        )
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
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let state = app_state(vault);
    let mut request_headers = HeaderMap::new();
    request_headers.insert(
        "x-codex-inference-call-id",
        HeaderValue::from_static("initial-inference-call"),
    );
    let response = call_core_with_headers(
        &state,
        &metadata.account_ref,
        Bytes::from_static(br#"{"model":"gpt-5.4","input":"hello","tools":[],"stream":false}"#),
        request_headers,
    )
    .await
    .expect("core response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024)
        .await
        .expect("response body");
    let downstream_response: serde_json::Value =
        serde_json::from_slice(&body).expect("downstream response JSON");
    let downstream_response_id = downstream_response["id"]
        .as_str()
        .expect("downstream response id");
    assert_ne!(downstream_response_id, "resp_refreshed");
    let (_, response_uuid) = downstream_response_id
        .split_once('_')
        .expect("prefixed response id");
    assert_eq!(
        uuid::Uuid::parse_str(response_uuid)
            .expect("response UUID")
            .get_version_num(),
        7
    );
    assert_eq!(mock_state.inference_calls.load(Ordering::SeqCst), 2);
    assert_eq!(mock_state.refresh_calls.load(Ordering::SeqCst), 1);
    let fingerprint_headers = mock_state.fingerprint_headers.lock().await;
    assert!(fingerprint_headers.is_empty());
    let inference_call_ids = mock_state.inference_call_ids.lock().await;
    assert_eq!(
        inference_call_ids[0].as_deref(),
        Some("initial-inference-call")
    );
    let retry_id = inference_call_ids[1]
        .as_deref()
        .expect("retry inference id");
    assert_eq!(
        uuid::Uuid::parse_str(retry_id)
            .expect("retry inference UUID")
            .get_version_num(),
        4
    );
    let bodies = mock_state.bodies.lock().await;
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0], bodies[1]);
    let decoded = zstd::stream::decode_all(std::io::Cursor::new(bodies[0].as_ref()))
        .expect("decompress replay body");
    let decoded: serde_json::Value = serde_json::from_slice(&decoded).expect("request JSON");
    assert_eq!(decoded["store"], false);
    assert_eq!(decoded["stream"], true);
    assert_eq!(
        uuid::Uuid::parse_str(
            decoded["client_metadata"]["x-codex-installation-id"]
                .as_str()
                .expect("installation")
        )
        .expect("installation UUID")
        .get_version_num(),
        4
    );
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
        fingerprint_headers: Arc::new(Mutex::new(Vec::new())),
        inference_call_ids: Arc::new(Mutex::new(Vec::new())),
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let app =
        Router::new()
            .route(
                "/responses",
                axum_post(
                    |AxumState(state): AxumState<OAuthMockState>,
                     headers: HeaderMap,
                     body: Bytes| async move {
                        state.inference_calls.fetch_add(1, Ordering::SeqCst);
                        state.bodies.lock().await.push(body);
                        if let Some(device) = header_text(&headers, "x-codex-installation-id") {
                            state.fingerprint_headers.lock().await.push(device);
                        }
                        state
                            .inference_call_ids
                            .lock()
                            .await
                            .push(header_text(&headers, "x-codex-inference-call-id"));
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
                        (
                            StatusCode::OK,
                            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_refreshed\",\"object\":\"response\"}}\n\n",
                        )
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
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let state = app_state(vault);
    let body =
        Bytes::from_static(br#"{"model":"gpt-5.4","input":"hello","tools":[],"stream":false}"#);
    let (first, second) = tokio::join!(
        call_core(&state, &metadata.account_ref, body.clone()),
        call_core(&state, &metadata.account_ref, body),
    );
    assert_eq!(first.expect("first response").status(), StatusCode::OK);
    assert_eq!(second.expect("second response").status(), StatusCode::OK);
    assert_eq!(mock_state.inference_calls.load(Ordering::SeqCst), 4);
    assert_eq!(mock_state.refresh_calls.load(Ordering::SeqCst), 1);
    let fingerprint_headers = mock_state.fingerprint_headers.lock().await;
    assert!(fingerprint_headers.is_empty());
}
