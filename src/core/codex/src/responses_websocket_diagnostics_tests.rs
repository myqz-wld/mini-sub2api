use super::*;
use mini_sub2api_protocol_v1::PROVIDER_REQUEST_ID_EVENT_TYPE;
use mini_sub2api_protocol_v1::PROVIDER_REQUEST_ID_HEADER;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn bare_upgrade_aliases_request_headers_and_keeps_one_private_diagnostic() {
    let upstream = spawn_loopback(Router::new().route(
        "/responses",
        get(|upgrade: WebSocketUpgrade| async move {
            let mut response = upgrade
                .on_upgrade(|mut socket| async move {
                    let _ = socket.close().await;
                })
                .into_response();
            response
                .headers_mut()
                .insert("x-request-id", HeaderValue::from_static("provider-bare-ws"));
            response.headers_mut().insert(
                "openai-request-id",
                HeaderValue::from_static("provider-bare-secondary"),
            );
            response.headers_mut().insert(
                "session-id",
                HeaderValue::from_static("session-must-not-cross"),
            );
            response.headers_mut().insert(
                "x-provider-future-id",
                HeaderValue::from_static("unknown-must-not-cross"),
            );
            response
        }),
    ))
    .await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let core = spawn_internal(state).await;

    let handshake = internal_handshake(&core.base_url, &account_ref)
        .upgrade()
        .send()
        .await
        .expect("internal handshake");
    assert_eq!(handshake.status(), StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(handshake.headers()["x-request-id"], "req_ws_test");
    assert_eq!(handshake.headers()["openai-request-id"], "req_ws_test");
    assert_eq!(
        handshake.headers()[PROVIDER_REQUEST_ID_HEADER],
        "provider-bare-ws"
    );
    assert!(!handshake.headers().contains_key("session-id"));
    assert!(!handshake.headers().contains_key("x-provider-future-id"));
}

#[tokio::test]
async fn deferred_codex_rejection_sends_private_diagnostic_before_structured_close() {
    let calls = Arc::new(AtomicUsize::new(0));
    let upstream = spawn_loopback(Router::new().route(
        "/responses",
        get({
            let calls = Arc::clone(&calls);
            move || {
                let calls = Arc::clone(&calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    AxumResponse::builder()
                        .status(StatusCode::TOO_MANY_REQUESTS)
                        .header("x-request-id", "provider-rejected-ws")
                        .header("session-id", "session-must-not-cross")
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            r#"{"error":{"response_id":"resp_must_not_cross"}}"#,
                        ))
                        .expect("rejection")
                }
            }
        }),
    ))
    .await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let core = spawn_internal(state).await;
    let mut socket = internal_handshake(&core.base_url, &account_ref)
        .header("originator", "codex_exec")
        .upgrade()
        .send()
        .await
        .expect("deferred internal handshake")
        .into_websocket()
        .await
        .expect("deferred internal socket");

    socket
        .send(DownstreamMessage::Text(
            serde_json::json!({
                "type":"response.create",
                "model":"gpt-5.4",
                "input":[{"type":"message","role":"user","content":"hello"}]
            })
            .to_string(),
        ))
        .await
        .expect("send create");
    let control = socket
        .next()
        .await
        .expect("diagnostic control")
        .expect("valid diagnostic control");
    let DownstreamMessage::Text(control) = control else {
        panic!("expected private diagnostic text")
    };
    let control: mini_sub2api_protocol_v1::ProviderRequestIdControl =
        serde_json::from_str(&control).expect("diagnostic JSON");
    assert_eq!(control.event_type, PROVIDER_REQUEST_ID_EVENT_TYPE);
    assert_eq!(control.provider_request_id, "provider-rejected-ws");

    let close = socket
        .next()
        .await
        .expect("failure close")
        .expect("valid failure close");
    let DownstreamMessage::Close { code, reason } = close else {
        panic!("expected failure close")
    };
    assert_eq!(
        u16::from(code),
        mini_sub2api_protocol_v1::FAILURE_CLOSE_CODE
    );
    let failure: mini_sub2api_protocol_v1::FailureMetadata =
        serde_json::from_str(&reason).expect("failure metadata");
    assert_eq!(
        failure,
        crate::error::failure(
            mini_sub2api_protocol_v1::RetryAdvice::Never,
            mini_sub2api_protocol_v1::FailurePhase::UpstreamResponse,
            mini_sub2api_protocol_v1::DeliveryState::NotDelivered,
        )
    );
    assert!(!reason.contains("resp_must_not_cross"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
