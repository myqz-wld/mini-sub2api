use super::*;
use crate::test_support::spawn_loopback;
use axum::routing::post;
use bytes::Bytes;
use http::HeaderValue;
use http_body_util::BodyExt;

#[tokio::test]
async fn all_api_key_callers_bypass_corrupt_state_and_keep_request_and_response_bytes() {
    const REQUEST: &[u8] = br#" {"model":"gpt-5.6-sol", "previous_response_id":"unmapped", "instructions":" {{caller}} ", "input":[{"role":"system","content":"keep"}], "store":true,"stream":false,"future":{"x":1},"client_metadata":{"session_id":37}} "#;
    const RESPONSE: &str = " {\"id\":\"resp_provider\", \"output\":[],\"future\":true} ";
    let upstream = spawn_loopback(Router::new().route(
        "/responses",
        post(|body: Bytes| async move {
            assert_eq!(body.as_ref(), REQUEST, "API key request bytes changed");
            Response::builder()
                .header("content-type", "application/json")
                .body(Body::from(RESPONSE))
                .expect("mock response")
        }),
    ))
    .await;
    let (state, account_ref, _temp) =
        super::integration_support::api_key_state(&upstream.base_url).await;
    let path = state
        .vault
        .request_state()
        .state_path_for_test(&account_ref);
    std::fs::write(&path, b"{corrupt").expect("broken identity state");
    for codex in [false, true] {
        let mut headers = HeaderMap::new();
        if codex {
            headers.insert("originator", HeaderValue::from_static("codex_exec"));
        }
        let response = super::integration_support::call_core_with_headers(
            &state,
            &account_ref,
            Bytes::from_static(REQUEST),
            headers,
        )
        .await
        .expect("API key request bypasses identity state");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        assert_eq!(body.as_ref(), RESPONSE.as_bytes(), "response bytes changed");
    }
    assert_eq!(std::fs::read(path).expect("state"), b"{corrupt");
}
