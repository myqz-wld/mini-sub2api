use super::*;
use crate::test_support::spawn_loopback;
use crate::test_support::test_jwt;
use axum::routing::post as axum_post;
use bytes::Bytes;
use http::HeaderValue;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use super::integration_support::api_key_state;
use super::integration_support::app_state;
use super::integration_support::call_core;
use super::integration_support::call_core_with_headers;

fn assert_safe_state_failure(error: CoreFailure, calls: &AtomicUsize) {
    assert!(matches!(error, CoreFailure::StateUnavailable));
    assert_eq!(error.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error.failure(),
        crate::error::failure(
            mini_sub2api_protocol_v1::RetryAdvice::Safe,
            mini_sub2api_protocol_v1::FailurePhase::Internal,
            mini_sub2api_protocol_v1::DeliveryState::NotDelivered,
        )
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn codex_api_key_missing_previous_response_fails_before_upstream_delivery() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/responses",
        axum_post({
            let calls = Arc::clone(&calls);
            move || {
                let calls = Arc::clone(&calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }
        }),
    );
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("codex_exec"));
    let error = call_core_with_headers(
        &state,
        &account_ref,
        Bytes::from_static(
            br#"{"model":"gpt-5.4","previous_response_id":"resp_missing","input":"hello"}"#,
        ),
        headers,
    )
    .await
    .expect_err("missing response mapping must fail closed");
    assert_safe_state_failure(error, &calls);
}

#[tokio::test]
async fn subscription_missing_previous_response_fails_before_upstream_delivery() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/responses",
        axum_post({
            let calls = Arc::clone(&calls);
            move || {
                let calls = Arc::clone(&calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }
        }),
    );
    let upstream = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let account_id = "chatgpt-missing-reference-http";
    let metadata = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: test_jwt(Some(account_id), 7200),
                access_token: test_jwt(None, 7200),
                refresh_token: "refresh-missing-reference".to_string(),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(2)),
                issuer: upstream.base_url.clone(),
                client_id: "client-missing-reference".to_string(),
            },
            format!("{}/responses", upstream.base_url),
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let error = call_core(
        &app_state(vault),
        &metadata.account_ref,
        Bytes::from_static(
            br#"{"model":"gpt-5.4","previous_response_id":"resp_missing","input":"hello"}"#,
        ),
    )
    .await
    .expect_err("missing response mapping must fail closed");
    assert_safe_state_failure(error, &calls);
}
