use super::*;
use crate::test_support::spawn_loopback;
use axum::{Router, body::Body, routing::get};
use http::StatusCode;
use http_body_util::BodyExt;
use serde_json::json;

#[tokio::test]
async fn http_flex_category_is_bounded_private_and_subscription_only() {
    for (status, body, expected) in [
        (StatusCode::BAD_REQUEST, r#"{"error":{"code":"invalid_prompt","message":"private-synthetic"}}"#.to_string(), "invalid_prompt"),
        (StatusCode::TOO_MANY_REQUESTS, r#"{"error":{"code":"flex_unavailable","message":"private-synthetic","debug":"private-synthetic"}}"#.to_string(), "flex_unavailable"),
        (StatusCode::BAD_REQUEST, r#"{"error":{"code":"flex_unavailable"}}"#.to_string(), "upstream_response_failed"),
        (StatusCode::TOO_MANY_REQUESTS, r#"{"error":{"code":"unrecognized-synthetic"}}"#.to_string(), "upstream_response_failed"),
        (StatusCode::TOO_MANY_REQUESTS, r#"{"error":{"code":"flex_unavailable"}} trailing"#.to_string(), "upstream_response_failed"),
        (StatusCode::TOO_MANY_REQUESTS, "x".repeat(64 * 1024 + 1), "upstream_response_failed"),
    ] {
        let upstream_body = body.clone();
        let upstream = spawn_loopback(Router::new().route("/", get(move || {
            let body = upstream_body.clone();
            async move { (status, [("retry-after", "9")], body) }
        }))).await;
        for subscription in [true, false] {
            let response = reqwest::Client::builder().no_proxy().build().unwrap()
                .get(&upstream.base_url).send().await.unwrap();
            let profile = if subscription {
                crate::request_profile::UpstreamProfile::CodexSubscription1580
            } else {
                crate::request_profile::UpstreamProfile::ApiKeyPassthrough
            };
            let response = crate::response_stream::build_http_response(
                response, 0, false, profile, None, "synthetic-request",
            ).await.unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()["retry-after"], "9");
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            if subscription {
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(value["error"]["code"], expected);
                assert_eq!(value["error"]["retryAdvice"], "never");
                assert!(!String::from_utf8_lossy(&bytes).contains("private-synthetic"));
            } else {
                assert_eq!(bytes.as_ref(), body.as_bytes());
            }
        }
    }
}

#[tokio::test]
async fn non_streaming_flex_event_finishes_without_waiting_for_eof() {
    let upstream = spawn_loopback(Router::new().route("/", get(|| async {
        let chunks = futures_util::stream::once(async {
            Ok::<_, std::convert::Infallible>("data: {\"type\":\"error\",\"error\":{\"code\":\"flex_unavailable\",\"message\":\"private-synthetic\"}}\n\n")
        }).chain(futures_util::stream::pending());
        ([("content-type", "text/event-stream")], Body::from_stream(chunks))
    }))).await;
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(&upstream.base_url)
        .send()
        .await
        .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        crate::response_stream::build_http_response(
            response,
            0,
            false,
            crate::request_profile::UpstreamProfile::CodexSubscription1580,
            None,
            "synthetic-request",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.status(), StatusCode::TOO_MANY_REQUESTS);
    let bytes = result.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["error"]["code"], "flex_unavailable");
    assert!(!String::from_utf8_lossy(&bytes).contains("private-synthetic"));
}

#[test]
fn flex_detection_distinguishes_error_events_from_business_content() {
    for (value, expected) in [
        (
            json!({"type":"error","error":{"code":"flex_unavailable"}}),
            true,
        ),
        (
            json!({"type":"error","error":null,"code":"flex_unavailable"}),
            true,
        ),
        (
            json!({"type":"response.failed","response":{"error":{"code":"flex_unavailable"}}}),
            true,
        ),
        (json!({"type":"error","error":{"code":"unknown"}}), false),
        (
            json!({"type":"response.output_text.delta","delta":"flex_unavailable"}),
            false,
        ),
    ] {
        assert_eq!(is_flex_event(&value), expected);
        assert_eq!(is_flex_sse(&value.to_string()), expected);
    }
}
