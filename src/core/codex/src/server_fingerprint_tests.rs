use super::*;
use crate::fingerprint::FingerprintMode;
use crate::test_support::spawn_loopback;
use axum::extract::State as AxumState;
use axum::routing::post as axum_post;
use bytes::Bytes;
use http::HeaderValue;

use super::integration_support::api_key_state_with_mode;
use super::integration_support::call_core_with_headers;

#[derive(Clone, Default)]
struct ProjectionCapture {
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
                    capture.headers.lock().await.push(headers);
                    capture.bodies.lock().await.push(body);
                    (StatusCode::OK, "captured")
                },
            ),
        )
        .with_state(capture)
}

#[tokio::test]
async fn api_key_routes_preserve_identity_and_body_bytes_in_every_fingerprint_mode() {
    for mode in [FingerprintMode::Off, FingerprintMode::Device] {
        let capture = ProjectionCapture::default();
        let upstream = spawn_loopback(capture_router(capture.clone())).await;
        let (state, account_ref, _temp) = api_key_state_with_mode(&upstream.base_url, mode).await;
        let original = Bytes::from_static(
            br#" {"client_metadata":{"x-codex-installation-id":"body-device","x-codex-turn-metadata":"{\"installation_id\":\"body-turn-device\"}"}} "#,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-installation-id",
            HeaderValue::from_static("header-device"),
        );
        headers.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_static(r#"{"installation_id":"header-device","future":true}"#),
        );

        call_core_with_headers(&state, &account_ref, original.clone(), headers)
            .await
            .expect("core response");
        assert_eq!(capture.bodies.lock().await[0], original);
        let captured = capture.headers.lock().await;
        assert_eq!(
            captured[0]
                .get("x-codex-installation-id")
                .and_then(|value| value.to_str().ok()),
            Some("header-device")
        );
        assert_eq!(
            captured[0]
                .get("x-codex-turn-metadata")
                .and_then(|value| value.to_str().ok()),
            Some(r#"{"installation_id":"header-device","future":true}"#)
        );
    }
}

#[tokio::test]
async fn api_key_device_mode_does_not_parse_or_recompress_caller_payloads() {
    let capture = ProjectionCapture::default();
    let upstream = spawn_loopback(capture_router(capture.clone())).await;
    let (state, account_ref, _temp) =
        api_key_state_with_mode(&upstream.base_url, FingerprintMode::Device).await;
    let opaque = Bytes::from_static(b"opaque-zstd-placeholder");
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::CONTENT_ENCODING,
        HeaderValue::from_static("zstd"),
    );

    call_core_with_headers(&state, &account_ref, opaque.clone(), headers)
        .await
        .expect("core response");
    assert_eq!(capture.bodies.lock().await[0], opaque);
    assert_eq!(
        capture.headers.lock().await[0]
            .get(http::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("zstd")
    );
}
