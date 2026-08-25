use super::*;
use mini_sub2api_protocol_v1::DeliveryState;
use mini_sub2api_protocol_v1::FAILURE_CLOSE_CODE;
use mini_sub2api_protocol_v1::FailureMetadata;
use mini_sub2api_protocol_v1::FailurePhase;
use mini_sub2api_protocol_v1::RetryAdvice;
use pretty_assertions::assert_eq;

#[derive(Clone, Copy)]
enum ProviderExit {
    BeforeEvent,
    AfterEvent,
}

#[tokio::test]
async fn relay_failure_close_tracks_attempted_and_observed_delivery() {
    for (provider_exit, expected_advice, expected_delivery) in [
        (
            ProviderExit::BeforeEvent,
            RetryAdvice::Ambiguous,
            DeliveryState::PossiblyDelivered,
        ),
        (
            ProviderExit::AfterEvent,
            RetryAdvice::Never,
            DeliveryState::Delivered,
        ),
    ] {
        let app = Router::new().route(
            "/responses",
            get(move |upgrade: WebSocketUpgrade| async move {
                upgrade
                    .on_upgrade(move |mut socket| async move {
                        let Some(Ok(InternalMessage::Text(_))) = socket.next().await else {
                            return;
                        };
                        if matches!(provider_exit, ProviderExit::AfterEvent) {
                            let _ = socket
                                .send(InternalMessage::Text(
                                    r#"{"type":"response.created","response":{"id":"resp_test"}}"#
                                        .into(),
                                ))
                                .await;
                        }
                    })
                    .into_response()
            }),
        );
        let upstream = spawn_loopback(app).await;
        let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
        let core = spawn_internal(state).await;
        let handshake = internal_handshake(&core.base_url, &account_ref)
            .upgrade()
            .send()
            .await
            .expect("internal handshake");
        let mut socket = handshake.into_websocket().await.expect("internal socket");
        socket
            .send(DownstreamMessage::Text(
                r#"{"type":"response.create","model":"test"}"#.to_string(),
            ))
            .await
            .expect("create frame");
        if matches!(provider_exit, ProviderExit::AfterEvent) {
            assert!(matches!(
                socket.next().await.expect("event").expect("valid event"),
                DownstreamMessage::Text(_)
            ));
        }
        let closed = socket
            .next()
            .await
            .expect("failure close")
            .expect("valid close");
        let DownstreamMessage::Close { code, reason } = closed else {
            panic!("expected failure close, got {closed:?}");
        };
        assert_eq!(u16::from(code), FAILURE_CLOSE_CODE);
        let metadata: FailureMetadata = serde_json::from_str(&reason).expect("failure metadata");
        assert_eq!(metadata.retry_advice, expected_advice);
        assert_eq!(metadata.phase, FailurePhase::WebSocketRelay);
        assert_eq!(metadata.delivery_state, expected_delivery);
    }
}
