use super::*;

#[tokio::test]
async fn codex_api_key_missing_previous_response_closes_before_provider_handshake() {
    let capture = WebSocketCapture::default();
    let app = Router::new()
        .route("/responses", get(accepting_upstream))
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let core = spawn_internal(state).await;
    let handshake = internal_handshake(&core.base_url, &account_ref)
        .header("originator", "codex_exec")
        .upgrade()
        .send()
        .await
        .expect("internal handshake");
    let mut socket = handshake.into_websocket().await.expect("internal socket");
    socket
        .send(DownstreamMessage::Text(
            serde_json::json!({
                "type":"response.create",
                "model":"gpt-5.4",
                "previous_response_id":"resp_missing",
                "input":"hello"
            })
            .to_string(),
        ))
        .await
        .expect("missing-reference create");
    let close = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("failure timeout")
        .expect("failure close")
        .expect("valid close");
    let DownstreamMessage::Close { code, reason } = close else {
        panic!("expected failure close, got {close:?}");
    };
    std::assert_eq!(
        u16::from(code),
        mini_sub2api_protocol_v1::FAILURE_CLOSE_CODE
    );
    let metadata: mini_sub2api_protocol_v1::FailureMetadata =
        serde_json::from_str(&reason).expect("failure metadata");
    std::assert_eq!(
        metadata,
        crate::error::failure(
            mini_sub2api_protocol_v1::RetryAdvice::Safe,
            mini_sub2api_protocol_v1::FailurePhase::Internal,
            mini_sub2api_protocol_v1::DeliveryState::NotDelivered,
        )
    );
    std::assert_eq!(capture.calls.load(Ordering::SeqCst), 0);
    assert!(capture.frames.lock().await.is_empty());
}
