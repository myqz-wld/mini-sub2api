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
async fn subscription_route_streams_upstream_and_aggregates_for_non_streaming_caller() {
    let capture = ApiCapture::default();
    let app = Router::new()
        .route(
            "/responses",
            axum_post(
                |AxumState(capture): AxumState<ApiCapture>,
                 headers: HeaderMap,
                 body: Bytes| async move {
                    *capture.headers.lock().await = Some(headers);
                    *capture.body.lock().await = Some(body);
                    AxumResponse::builder()
                        .status(StatusCode::OK)
                        .header(http::header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(
                            "event: response.output_text.delta\r\n\
                             data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"item_id\":\"msg_answer\",\"delta\":\"ok\"}\r\n\r\n\
                             event: response.completed\r\n\
                             data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_api_key\",\"object\":\"response\",\"output\":[{\"type\":\"message\",\"id\":\"msg_answer\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"}]}]}}\r\n\r\n",
                        ))
                        .expect("mock response")
                },
            ),
        )
        .with_state(capture.clone());
    let mock = spawn_loopback(app).await;
    let (state, account_ref, _temp) =
        super::integration_support::subscription_state(&mock.base_url).await;
    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("codex_exec"));

    let response = call_core_with_headers(
        &state,
        &account_ref,
        Bytes::from_static(br#"{"model":"gpt-5.4","input":[],"store":true}"#),
        headers,
    )
    .await
    .expect("core response");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    let returned = serde_json::from_slice::<serde_json::Value>(&body).expect("response JSON");
    let response_id = returned["id"].as_str().expect("response ID");
    assert_ne!(response_id, "resp_api_key");
    assert_eq!(returned["object"], "response");
    assert_eq!(returned["output"].as_array().unwrap().len(), 1);
    assert_eq!(returned["output"][0]["content"][0]["text"], "ok");
    assert_eq!(
        uuid::Uuid::parse_str(
            response_id
                .strip_prefix("resp_")
                .expect("response alias prefix")
        )
        .expect("response alias UUID")
        .get_version_num(),
        7
    );

    let upstream: serde_json::Value = serde_json::from_slice(
        &zstd::stream::decode_all(capture.body.lock().await.as_deref().expect("captured body"))
            .expect("zstd body"),
    )
    .expect("upstream JSON");
    assert_eq!(upstream["store"], false);
    assert_eq!(upstream["stream"], true);
    assert_eq!(
        header_text(
            capture
                .headers
                .lock()
                .await
                .as_ref()
                .expect("captured headers"),
            "accept",
        )
        .as_deref(),
        Some("text/event-stream")
    );
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

#[path = "server_subscription_request_tests.rs"]
mod subscription_request_tests;
