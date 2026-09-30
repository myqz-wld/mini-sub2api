use super::*;
use http_body_util::BodyExt;
use serde_json::json;

fn window_cases() -> Vec<(Option<Value>, Option<u16>)> {
    vec![
        (None, None),
        (Some(json!(null)), None),
        (Some(json!(0)), Some(0)),
        (Some(json!(300)), Some(300)),
        (Some(json!(10080)), Some(10080)),
        (Some(json!(65535)), Some(65535)),
        (Some(json!(-1)), None),
        (Some(json!(65536)), None),
        (Some(json!(1.5)), None),
        (Some(json!(300.0)), None),
        (Some(json!(u64::MAX)), None),
        (Some(json!("300")), None),
        (Some(json!(true)), None),
        (Some(json!({"minutes":300})), None),
        (Some(json!([300])), None),
    ]
}

fn provider_error(window: Option<Value>, kind: &str) -> Value {
    let mut error = json!({"type":kind,"message":"synthetic-private",
        "plan_type":"synthetic-private","debug":"synthetic-private"});
    if let Some(window) = window {
        error["limit_window_minutes"] = window;
    }
    error
}

#[tokio::test]
async fn usage_window_http_category_and_protocol_keep_bounds_without_provider_fields() {
    for (window, expected) in window_cases() {
        let body = json!({"error":provider_error(window, "usage_limit_reached")});
        let failure = http_category(
            http::StatusCode::TOO_MANY_REQUESTS,
            body.to_string().as_bytes(),
        )
        .expect("usage-limit category survives malformed optional metadata");
        assert_eq!(failure.code(), "usage_limit_reached");
        assert_eq!(failure.native_error_type(), Some("usage_limit_reached"));
        assert_eq!(failure.limit_window_minutes(), expected);
        let response = failure.into_response("req_synthetic".into());
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["error"]["code"], "usage_limit_reached");
        assert_eq!(value["error"]["limitWindowMinutes"], json!(expected));
        assert_eq!(
            value["error"].get("limitWindowMinutes").is_some(),
            expected.is_some()
        );
        assert!(!value.to_string().contains("synthetic-private"));
    }
}

#[test]
fn usage_window_requires_native_type_and_does_not_change_error_classification() {
    for (kind, code, expected) in [
        (
            "usage_not_included",
            "usage_limit_reached",
            "usage_not_included",
        ),
        (
            "insufficient_quota",
            "usage_limit_reached",
            "insufficient_quota",
        ),
        (
            "usage_limit_reached",
            "flex_unavailable",
            "flex_unavailable",
        ),
    ] {
        let mut error = provider_error(Some(json!(300)), kind);
        error["code"] = code.into();
        let failure = http_category(
            http::StatusCode::TOO_MANY_REQUESTS,
            json!({"error":error}).to_string().as_bytes(),
        )
        .unwrap();
        assert_eq!(failure.code(), expected);
        assert_eq!(failure.limit_window_minutes(), None);
    }
    for kind in [
        None,
        Some("usage_not_included"),
        Some("Usage_Limit_Reached"),
    ] {
        assert_eq!(usage_limit_window(kind, Some(&json!(300))), None);
    }
    let body = br#"{"error":{"code":"usage_limit_reached","limit_window_minutes":300}}"#;
    assert!(http_category(http::StatusCode::TOO_MANY_REQUESTS, body).is_none());
}

#[test]
fn usage_window_stream_privacy_keeps_only_matching_bounded_metadata() {
    for kind in [
        "usage_limit_reached",
        "usage_not_included",
        "insufficient_quota",
    ] {
        for (window, expected) in window_cases() {
            let expected = if kind == "usage_limit_reached" {
                expected
            } else {
                None
            };
            let error = provider_error(window, kind);
            for (mut event, pointer) in [
                (json!({"type":"error","status":429,"error":error}), "/error"),
                (
                    json!({"type":"response.failed","response":{"error":error}}),
                    "/response/error",
                ),
                (json!({"error":error}), "/error"),
            ] {
                crate::response_privacy::filter_response(&mut event, "req_synthetic");
                let public = event.pointer(pointer).unwrap();
                assert_eq!(public["type"], kind);
                assert_eq!(public["limit_window_minutes"], json!(expected));
                assert_eq!(
                    public.get("limit_window_minutes").is_some(),
                    expected.is_some()
                );
                assert_eq!(
                    public.as_object().unwrap().len(),
                    3 + usize::from(expected.is_some())
                );
                assert!(!event.to_string().contains("synthetic-private"));
            }
        }
    }
    let mut flat = json!({"type":"error","code":"usage_limit_reached",
        "limit_window_minutes":300,"message":"synthetic-private"});
    crate::response_privacy::filter_response(&mut flat, "req_synthetic");
    assert!(flat.get("limit_window_minutes").is_none());
}
