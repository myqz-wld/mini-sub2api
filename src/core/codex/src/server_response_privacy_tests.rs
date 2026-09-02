use super::*;
use crate::test_support::spawn_loopback;
use crate::test_support::test_jwt;
use axum::Json;
use axum::extract::State as AxumState;
use axum::response::Response as AxumResponse;
use axum::routing::post as axum_post;
use bytes::Bytes;
use http::HeaderValue;
use http_body_util::BodyExt;
use mini_sub2api_protocol_v1::PROVIDER_REQUEST_ID_HEADER;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use super::integration_support::api_key_state;
use super::integration_support::app_state;
use super::integration_support::call_core_with_headers;

#[derive(Clone, Default)]
struct TranslationFailureState {
    state_path: Arc<StdMutex<Option<std::path::PathBuf>>>,
}

fn private_upstream_response(status: StatusCode, body: &'static str) -> AxumResponse {
    AxumResponse::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain")
        .header(http::header::CONTENT_ENCODING, "identity")
        .header("retry-after", "9")
        .header("server-timing", "provider;dur=8")
        .header("openai-model", "gpt-private")
        .header("x-codex-turn-state", "opaque-turn-state")
        .header("x-request-id", "provider-primary")
        .header("openai-request-id", "provider-secondary")
        .header("session-id", "session-must-not-cross")
        .header("x-provider-future-id", "unknown-must-not-cross")
        .body(Body::from(body))
        .expect("mock response")
}

#[tokio::test]
async fn codex_failure_normalizes_body_and_default_denies_response_headers() {
    let raw_body = r#"provider response resp_raw conv_raw request_raw"#;
    let upstream = spawn_loopback(Router::new().route(
        "/responses",
        axum_post(move || async move {
            private_upstream_response(StatusCode::TOO_MANY_REQUESTS, raw_body)
        }),
    ))
    .await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("codex_exec"));
    let response = call_core_with_headers(
        &state,
        &account_ref,
        Bytes::from_static(br#"{"model":"gpt-5.4","input":"hello","stream":true}"#),
        headers,
    )
    .await
    .expect("normalized response");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["x-request-id"], "req_test");
    assert_eq!(response.headers()["openai-request-id"], "req_test");
    assert_eq!(
        response.headers()[PROVIDER_REQUEST_ID_HEADER],
        "provider-primary"
    );
    assert_eq!(response.headers()["retry-after"], "9");
    assert_eq!(response.headers()["server-timing"], "provider;dur=8");
    assert_eq!(response.headers()["openai-model"], "gpt-private");
    assert_eq!(
        response.headers()["x-codex-turn-state"],
        "opaque-turn-state"
    );
    assert!(!response.headers().contains_key("session-id"));
    assert!(!response.headers().contains_key("x-provider-future-id"));
    assert!(
        !response
            .headers()
            .contains_key(http::header::CONTENT_ENCODING)
    );
    assert_eq!(
        response.headers()[http::header::CONTENT_TYPE],
        "application/json"
    );

    let body = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).expect("gateway error JSON");
    assert_eq!(body["error"]["code"], "upstream_response_failed");
    assert_eq!(body["error"]["requestId"], "req_test");
    assert_eq!(body["error"]["retryAdvice"], "never");
    assert_eq!(body["error"]["phase"], "upstream_response");
    assert_eq!(body["error"]["deliveryState"], "delivered");
    assert!(!body.to_string().contains("resp_raw"));
    assert!(!body.to_string().contains("conv_raw"));
    assert!(!body.to_string().contains("request_raw"));
}

#[tokio::test]
async fn subscription_failure_uses_the_same_normalized_privacy_boundary() {
    let upstream = spawn_loopback(Router::new().route(
        "/responses",
        axum_post(|| async {
            private_upstream_response(
                StatusCode::TOO_MANY_REQUESTS,
                "subscription resp_raw conv_raw request_raw",
            )
        }),
    ))
    .await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let account_id = "chatgpt-response-privacy";
    let credential = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: test_jwt(Some(account_id), 7200),
                access_token: test_jwt(None, 7200),
                refresh_token: "refresh-response-privacy".to_string(),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(2)),
                issuer: upstream.base_url.clone(),
                client_id: "client-response-privacy".to_string(),
            },
            format!("{}/responses", upstream.base_url),
            FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let response = call_core_with_headers(
        &app_state(vault),
        &credential.account_ref,
        Bytes::from_static(br#"{"model":"gpt-5.4","input":"hello","stream":true}"#),
        HeaderMap::new(),
    )
    .await
    .expect("normalized subscription response");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["x-request-id"], "req_test");
    assert_eq!(
        response.headers()[PROVIDER_REQUEST_ID_HEADER],
        "provider-primary"
    );
    assert!(!response.headers().contains_key("session-id"));
    let body = response
        .into_body()
        .collect()
        .await
        .expect("subscription response body")
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).expect("gateway error JSON");
    assert_eq!(body["error"]["code"], "upstream_response_failed");
    assert_eq!(body["error"]["deliveryState"], "delivered");
    assert!(!body.to_string().contains("resp_raw"));
    assert!(!body.to_string().contains("conv_raw"));
    assert!(!body.to_string().contains("request_raw"));
}

#[tokio::test]
async fn final_oauth_unauthorized_keeps_only_the_second_private_diagnostic() {
    let inference_calls = Arc::new(AtomicUsize::new(0));
    let refresh_calls = Arc::new(AtomicUsize::new(0));
    let account_id = "chatgpt-response-auth-privacy";
    let new_access = test_jwt(None, 7200);
    let new_id = test_jwt(Some(account_id), 7200);
    let upstream = spawn_loopback(
        Router::new()
            .route(
                "/responses",
                axum_post({
                    let calls = Arc::clone(&inference_calls);
                    move || {
                        let calls = Arc::clone(&calls);
                        async move {
                            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                            AxumResponse::builder()
                                .status(StatusCode::UNAUTHORIZED)
                                .header("x-request-id", format!("provider-auth-{call}"))
                                .header("x-provider-future-id", "must-not-cross")
                                .body(Body::from(format!("raw unauthorized resp_{call}")))
                                .expect("unauthorized response")
                        }
                    }
                }),
            )
            .route(
                "/oauth/token",
                axum_post({
                    let calls = Arc::clone(&refresh_calls);
                    let new_access = new_access.clone();
                    let new_id = new_id.clone();
                    move || {
                        let calls = Arc::clone(&calls);
                        let new_access = new_access.clone();
                        let new_id = new_id.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            Json(serde_json::json!({
                                "id_token": new_id,
                                "access_token": new_access,
                                "refresh_token": "refresh-auth-privacy-new"
                            }))
                        }
                    }
                }),
            ),
    )
    .await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let credential = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: test_jwt(Some(account_id), 3600),
                access_token: test_jwt(None, 3600),
                refresh_token: "refresh-auth-privacy-old".to_string(),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                issuer: upstream.base_url.clone(),
                client_id: "client-auth-privacy".to_string(),
            },
            format!("{}/responses", upstream.base_url),
            FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let response = call_core_with_headers(
        &app_state(vault),
        &credential.account_ref,
        Bytes::from_static(br#"{"model":"gpt-5.4","input":"hello"}"#),
        HeaderMap::new(),
    )
    .await
    .expect("normalized final unauthorized response");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()["x-request-id"], "req_test");
    assert_eq!(
        response.headers()[PROVIDER_REQUEST_ID_HEADER],
        "provider-auth-2"
    );
    assert!(!response.headers().contains_key("x-provider-future-id"));
    let body = response
        .into_body()
        .collect()
        .await
        .expect("OAuth error body")
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).expect("OAuth gateway error JSON");
    assert_eq!(body["error"]["code"], "upstream_auth_failed");
    assert_eq!(body["error"]["deliveryState"], "delivered");
    assert!(!body.to_string().contains("resp_1"));
    assert!(!body.to_string().contains("resp_2"));
    assert_eq!(inference_calls.load(Ordering::SeqCst), 2);
    assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn aggregated_translation_failure_retains_private_provider_diagnostic() {
    let failure = TranslationFailureState::default();
    let upstream = spawn_loopback(
        Router::new()
            .route(
                "/responses",
                axum_post(|AxumState(state): AxumState<TranslationFailureState>| async move {
                    let path = state
                        .state_path
                        .lock()
                        .expect("state path lock")
                        .clone()
                        .expect("state path");
                    std::fs::write(path, b"{corrupt").expect("corrupt request state");
                    AxumResponse::builder()
                        .status(StatusCode::OK)
                        .header(http::header::CONTENT_TYPE, "text/event-stream")
                        .header("x-request-id", "provider-translation-failure")
                        .body(Body::from(
                            "event: response.completed\n\
                             data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_translation_failure\"}}\n\n",
                        ))
                        .expect("translation failure response")
                }),
            )
            .with_state(failure.clone()),
    )
    .await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    *failure.state_path.lock().expect("state path lock") = Some(
        state
            .vault
            .request_state()
            .state_path_for_test(&account_ref),
    );
    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("codex_exec"));
    let response = call_core_with_headers(
        &state,
        &account_ref,
        Bytes::from_static(br#"{"model":"gpt-5.4","input":[],"stream":false}"#),
        headers,
    )
    .await
    .expect("normalized translation failure");

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(response.headers()["x-request-id"], "req_test");
    assert_eq!(
        response.headers()[PROVIDER_REQUEST_ID_HEADER],
        "provider-translation-failure"
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("translation failure body")
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).expect("gateway error JSON");
    assert_eq!(body["error"]["code"], "upstream_response_failed");
    assert_eq!(body["error"]["retryAdvice"], "never");
    assert_eq!(body["error"]["deliveryState"], "delivered");
    assert!(!body.to_string().contains("resp_translation_failure"));
}

#[tokio::test]
async fn bare_failure_keeps_body_bytes_but_uses_the_same_header_privacy_policy() {
    let raw_body = "bare provider body resp_raw\n";
    let upstream = spawn_loopback(Router::new().route(
        "/responses",
        axum_post(
            move || async move { private_upstream_response(StatusCode::IM_A_TEAPOT, raw_body) },
        ),
    ))
    .await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let response = call_core_with_headers(
        &state,
        &account_ref,
        Bytes::from_static(br#"{"model":"bare","stream":true}"#),
        HeaderMap::new(),
    )
    .await
    .expect("bare response");

    assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(response.headers()["x-request-id"], "req_test");
    assert_eq!(response.headers()["openai-request-id"], "req_test");
    assert_eq!(
        response.headers()[PROVIDER_REQUEST_ID_HEADER],
        "provider-primary"
    );
    assert!(!response.headers().contains_key("session-id"));
    assert!(!response.headers().contains_key("x-provider-future-id"));
    let body = response
        .into_body()
        .collect()
        .await
        .expect("bare response body")
        .to_bytes();
    assert_eq!(body, raw_body.as_bytes());
}
