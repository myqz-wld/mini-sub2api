use super::*;
use crate::fingerprint::FingerprintMode;
use crate::test_support::spawn_loopback;
use axum::extract::State as AxumState;
use axum::routing::post as axum_post;
use bytes::Bytes;
use http::HeaderValue;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use super::integration_support::api_key_state;
use super::integration_support::api_key_state_with_mode;
use super::integration_support::app_state;
use super::integration_support::call_core;
use super::integration_support::call_core_with_headers;

#[derive(Clone, Default)]
struct ProjectionCapture {
    calls: Arc<AtomicUsize>,
    headers: Arc<Mutex<Vec<HeaderMap>>>,
    bodies: Arc<Mutex<Vec<Bytes>>>,
}

fn capture_router(capture: ProjectionCapture) -> Router {
    Router::new()
        .route(
            "/responses",
            axum_post(
                |AxumState(capture): AxumState<ProjectionCapture>,
                 headers: HeaderMap,
                 body: Bytes| async move {
                    capture.calls.fetch_add(1, Ordering::SeqCst);
                    capture.headers.lock().await.push(headers);
                    capture.bodies.lock().await.push(body);
                    (StatusCode::OK, "captured")
                },
            ),
        )
        .with_state(capture)
}

#[tokio::test]
async fn api_key_device_adds_header_without_rewriting_carrier_free_body() {
    let capture = ProjectionCapture::default();
    let upstream = spawn_loopback(capture_router(capture.clone())).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let expected_device = state
        .vault
        .fingerprint_snapshot(&account_ref)
        .await
        .expect("fingerprint")
        .installation_id()
        .to_string();
    let original = Bytes::from_static(br#" {"model":"test", "custom":"kept"} "#);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-codex-installation-id",
        HeaderValue::from_static("caller-device"),
    );

    let response = call_core_with_headers(&state, &account_ref, original.clone(), headers)
        .await
        .expect("core response");
    assert_eq!(response.status(), StatusCode::OK);
    let captured_headers = capture.headers.lock().await;
    assert!(
        captured_headers[0]
            .get("x-codex-installation-id")
            .and_then(|value| value.to_str().ok())
            == Some(expected_device.as_str())
    );
    assert_eq!(capture.bodies.lock().await[0], original);
}

#[tokio::test]
async fn api_key_device_converges_body_and_nested_turn_metadata() {
    let capture = ProjectionCapture::default();
    let upstream = spawn_loopback(capture_router(capture.clone())).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let expected_device = state
        .vault
        .fingerprint_snapshot(&account_ref)
        .await
        .expect("fingerprint")
        .installation_id()
        .to_string();
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "test",
            "client_metadata": {
                "x-codex-installation-id": "body-device",
                "x-codex-turn-metadata": serde_json::json!({
                    "installation_id": "body-turn-device",
                    "session_id": "session-kept",
                    "thread_id": "thread-kept",
                    "turn_id": "turn-kept",
                    "window_id": "window-kept",
                    "future": {"kept": true}
                }).to_string(),
                "custom": "kept"
            }
        }))
        .expect("body"),
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_static(
            r#"{"installation_id":"header-device","session_id":"header-session","future":1}"#,
        ),
    );

    call_core_with_headers(&state, &account_ref, body, headers)
        .await
        .expect("core response");
    let captured_headers = capture.headers.lock().await;
    let header_turn: serde_json::Value = serde_json::from_str(
        captured_headers[0]
            .get("x-codex-turn-metadata")
            .and_then(|value| value.to_str().ok())
            .expect("header turn metadata"),
    )
    .expect("header turn JSON");
    assert!(header_turn["installation_id"].as_str() == Some(expected_device.as_str()));
    assert_eq!(header_turn["session_id"], "header-session");
    assert_eq!(header_turn["future"], 1);

    let bodies = capture.bodies.lock().await;
    let body: serde_json::Value = serde_json::from_slice(&bodies[0]).expect("body JSON");
    assert!(
        body["client_metadata"]["x-codex-installation-id"].as_str()
            == Some(expected_device.as_str())
    );
    assert_eq!(body["client_metadata"]["custom"], "kept");
    let turn: serde_json::Value = serde_json::from_str(
        body["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("body turn metadata"),
    )
    .expect("body turn JSON");
    assert!(turn["installation_id"].as_str() == Some(expected_device.as_str()));
    assert_eq!(turn["session_id"], "session-kept");
    assert_eq!(turn["thread_id"], "thread-kept");
    assert_eq!(turn["turn_id"], "turn-kept");
    assert_eq!(turn["window_id"], "window-kept");
    assert_eq!(turn["future"]["kept"], true);
}

#[tokio::test]
async fn api_key_off_preserves_conflicting_caller_identity_bytes() {
    let capture = ProjectionCapture::default();
    let upstream = spawn_loopback(capture_router(capture.clone())).await;
    let (state, account_ref, _temp) =
        api_key_state_with_mode(&upstream.base_url, FingerprintMode::Off).await;
    let original = Bytes::from_static(
        br#" {"client_metadata":{"x-codex-installation-id":"body-device","x-codex-turn-metadata":"{\"installation_id\":\"body-turn-device\"}"}} "#,
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-codex-installation-id",
        HeaderValue::from_static("header-device"),
    );

    call_core_with_headers(&state, &account_ref, original.clone(), headers)
        .await
        .expect("core response");
    assert_eq!(capture.bodies.lock().await[0], original);
    assert_eq!(
        capture.headers.lock().await[0]
            .get("x-codex-installation-id")
            .and_then(|value| value.to_str().ok()),
        Some("header-device")
    );
}

#[tokio::test]
async fn unsafe_device_requests_fail_before_upstream_send() {
    let capture = ProjectionCapture::default();
    let upstream = spawn_loopback(capture_router(capture.clone())).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;

    let malformed = call_core(&state, &account_ref, Bytes::from_static(b"not-json")).await;
    assert!(matches!(malformed, Err(CoreFailure::InvalidRequest)));

    let mut encoded_headers = HeaderMap::new();
    encoded_headers.insert(
        http::header::CONTENT_ENCODING,
        HeaderValue::from_static("zstd"),
    );
    let encoded = call_core_with_headers(
        &state,
        &account_ref,
        Bytes::from_static(br#"{}"#),
        encoded_headers,
    )
    .await;
    assert!(matches!(encoded, Err(CoreFailure::InvalidRequest)));

    let mut metadata_headers = HeaderMap::new();
    metadata_headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_static("not-json"),
    );
    let malformed_metadata = call_core_with_headers(
        &state,
        &account_ref,
        Bytes::from_static(br#"{}"#),
        metadata_headers,
    )
    .await;
    assert!(matches!(
        malformed_metadata,
        Err(CoreFailure::InvalidRequest)
    ));

    assert_eq!(capture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn two_credentials_at_one_origin_expose_distinct_devices() {
    let capture = ProjectionCapture::default();
    let upstream = spawn_loopback(capture_router(capture.clone())).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let first = vault
        .create_api_key(
            "first-secret".to_string(),
            format!("{}/responses", upstream.base_url),
            FingerprintMode::Device,
        )
        .await
        .expect("first credential");
    let second = vault
        .create_api_key(
            "second-secret".to_string(),
            format!("{}/responses", upstream.base_url),
            FingerprintMode::Device,
        )
        .await
        .expect("second credential");
    let first_device = vault
        .fingerprint_snapshot(&first.account_ref)
        .await
        .expect("first fingerprint")
        .installation_id()
        .to_string();
    let second_device = vault
        .fingerprint_snapshot(&second.account_ref)
        .await
        .expect("second fingerprint")
        .installation_id()
        .to_string();
    let state = app_state(vault);

    for account_ref in [&first.account_ref, &second.account_ref] {
        call_core(
            &state,
            account_ref,
            Bytes::from_static(br#"{"model":"test"}"#),
        )
        .await
        .expect("core response");
    }
    let headers = capture.headers.lock().await;
    let observed = headers
        .iter()
        .map(|headers| {
            headers
                .get("x-codex-installation-id")
                .and_then(|value| value.to_str().ok())
                .expect("device header")
        })
        .collect::<Vec<_>>();
    assert!(observed[0] == first_device);
    assert!(observed[1] == second_device);
    assert!(observed[0] != observed[1]);
}
