use super::*;

#[tokio::test]
async fn missing_subscription_previous_response_closes_before_provider_connect() {
    let fixture = holding_oauth_fixture(
        "chatgpt-websocket-missing-reference",
        HoldingProviderEvent::None,
    )
    .await;
    let core = spawn_internal(app_state(fixture.vault.clone())).await;
    let handshake = internal_handshake(&core.base_url, &fixture.account_ref)
        .upgrade()
        .send()
        .await
        .expect("handshake");
    let mut socket = handshake.into_websocket().await.expect("socket");
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
    std::assert_eq!(
        failure_metadata(close),
        crate::error::failure(
            mini_sub2api_protocol_v1::RetryAdvice::Safe,
            mini_sub2api_protocol_v1::FailurePhase::Internal,
            mini_sub2api_protocol_v1::DeliveryState::NotDelivered,
        )
    );
    std::assert_eq!(fixture.state.handshake_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.state.frames.lock().await.is_empty());
}
