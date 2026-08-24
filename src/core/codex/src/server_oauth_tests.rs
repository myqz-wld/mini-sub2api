use super::*;
use crate::test_support::spawn_loopback;
use crate::test_support::test_jwt;
use axum::Json;
use axum::extract::State as AxumState;
use axum::routing::post as axum_post;
use bytes::Bytes;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use super::integration_support::app_state;
use super::integration_support::call_core;

#[derive(Clone)]
struct OAuthMockState {
    old_access: String,
    new_access: String,
    new_id: String,
    inference_calls: Arc<AtomicUsize>,
    refresh_calls: Arc<AtomicUsize>,
    old_token_barrier: Option<Arc<tokio::sync::Barrier>>,
    fingerprint_headers: Arc<Mutex<Vec<String>>>,
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
    };
    let app = Router::new()
        .route(
            "/responses",
            axum_post(
                |AxumState(state): AxumState<OAuthMockState>, headers: HeaderMap| async move {
                    state.inference_calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(
                        header_text(&headers, crate::upstream_request::CODEX_VERSION_HEADER)
                            .as_deref(),
                        Some(crate::upstream_request::CODEX_COMPATIBILITY_VERSION)
                    );
                    if let Some(device) = header_text(&headers, "x-codex-installation-id") {
                        state.fingerprint_headers.lock().await.push(device);
                    }
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
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let state = app_state(vault);
    let expected_device = state
        .vault
        .fingerprint_snapshot(&metadata.account_ref)
        .await
        .expect("fingerprint")
        .installation_id()
        .to_string();

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
    let fingerprint_headers = mock_state.fingerprint_headers.lock().await;
    assert_eq!(fingerprint_headers.len(), 2);
    assert!(
        fingerprint_headers
            .iter()
            .all(|observed| observed == &expected_device)
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
    };
    let app = Router::new()
        .route(
            "/responses",
            axum_post(
                |AxumState(state): AxumState<OAuthMockState>, headers: HeaderMap| async move {
                    state.inference_calls.fetch_add(1, Ordering::SeqCst);
                    if let Some(device) = header_text(&headers, "x-codex-installation-id") {
                        state.fingerprint_headers.lock().await.push(device);
                    }
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
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let state = app_state(vault);
    let expected_device = state
        .vault
        .fingerprint_snapshot(&metadata.account_ref)
        .await
        .expect("fingerprint")
        .installation_id()
        .to_string();
    let body = Bytes::from_static(br#"{"stream":false}"#);
    let (first, second) = tokio::join!(
        call_core(&state, &metadata.account_ref, body.clone()),
        call_core(&state, &metadata.account_ref, body),
    );
    assert_eq!(first.expect("first response").status(), StatusCode::OK);
    assert_eq!(second.expect("second response").status(), StatusCode::OK);
    assert_eq!(mock_state.inference_calls.load(Ordering::SeqCst), 4);
    assert_eq!(mock_state.refresh_calls.load(Ordering::SeqCst), 1);
    let fingerprint_headers = mock_state.fingerprint_headers.lock().await;
    assert_eq!(fingerprint_headers.len(), 4);
    assert!(
        fingerprint_headers
            .iter()
            .all(|observed| observed == &expected_device)
    );
}
