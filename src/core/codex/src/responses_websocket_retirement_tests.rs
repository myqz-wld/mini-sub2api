use super::*;
use serde_json::json;

fn fixture() -> (
    Arc<StdMutex<ResponsesWebSocketState>>,
    WebSocketDeliveryTracker,
) {
    let state = Arc::new(StdMutex::new(ResponsesWebSocketState::new(
        CallerKind::Codex,
        UpstreamProfile::ApiKeyPassthrough,
    )));
    let delivery = WebSocketDeliveryTracker::default();
    delivery.mark_attempted();
    (state, delivery)
}

#[tokio::test(start_paused = true)]
async fn expired_failure_tail_closes_even_with_a_ready_old_footer() {
    let (state, delivery) = fixture();
    let mut inbound = Inbound::default();
    inbound
        .translate(
            json!({"type":"response.created","response":{"id":"old"}}).to_string(),
            None,
            &state,
            &delivery,
        )
        .await
        .unwrap();
    inbound
        .translate(
            json!({"type":"error","error":{"code":"server_error"}}).to_string(),
            None,
            &state,
            &delivery,
        )
        .await
        .unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    let mut stream = futures_util::stream::iter(["late old footer"]);
    assert!(inbound.next(&mut stream).await.is_none());
    assert!(delivery.is_retired());
    assert_eq!(
        delivery.failure().retry_advice,
        mini_sub2api_protocol_v1::RetryAdvice::Never
    );
    assert_eq!(
        delivery.failure().delivery_state,
        mini_sub2api_protocol_v1::DeliveryState::Delivered
    );
}

#[tokio::test]
async fn failed_stream_rejects_unbound_frames_and_keeps_delivery_after_footer() {
    for with_created in [false, true] {
        for with_status in [false, true] {
            let (state, delivery) = fixture();
            let mut inbound = Inbound::default();
            if with_created {
                inbound
                    .translate(
                        json!({"type":"response.created","response":{"id":"old"}}).to_string(),
                        None,
                        &state,
                        &delivery,
                    )
                    .await
                    .unwrap();
            }
            let mut error = json!({"type":"error","error":{"code":"server_error"}});
            if with_status {
                error["status"] = 500.into();
            }
            inbound
                .translate(error.to_string(), None, &state, &delivery)
                .await
                .unwrap();
            assert!(delivery.is_retired());
            if !with_status {
                let footer = json!({"type":"response.failed","response":{"id":"old","output":[]}});
                let (_, terminal) = inbound
                    .translate(footer.to_string(), None, &state, &delivery)
                    .await
                    .unwrap()
                    .unwrap();
                delivery.mark_terminal(terminal.unwrap());
            }
            assert!(
                inbound
                    .next(&mut futures_util::stream::iter(["not reusable"]))
                    .await
                    .is_none()
            );
            assert_eq!(
                delivery.failure().retry_advice,
                mini_sub2api_protocol_v1::RetryAdvice::Never
            );
        }
    }
    let (state, delivery) = fixture();
    let mut inbound = Inbound::default();
    inbound
        .translate(
            json!({"type":"response.created","response":{"id":"old"}}).to_string(),
            None,
            &state,
            &delivery,
        )
        .await
        .unwrap();
    inbound
        .translate(json!({"type":"error"}).to_string(), None, &state, &delivery)
        .await
        .unwrap();
    assert!(
        inbound
            .translate(
                json!({"type":"response.failed"}).to_string(),
                None,
                &state,
                &delivery
            )
            .await
            .is_err()
    );
}

#[tokio::test(start_paused = true)]
async fn repeated_errors_cannot_extend_the_retired_stream_deadline() {
    let (state, delivery) = fixture();
    let mut inbound = Inbound::default();
    let error =
        json!({"type":"error","response_id":"old","error":{"code":"server_error"}}).to_string();
    inbound
        .translate(error.clone(), None, &state, &delivery)
        .await
        .unwrap();
    let generation = delivery.generation();
    tokio::time::advance(Duration::from_millis(900)).await;
    inbound
        .translate(error, None, &state, &delivery)
        .await
        .unwrap();
    tokio::time::advance(Duration::from_millis(200)).await;
    assert!(
        inbound
            .next(&mut futures_util::stream::pending::<()>())
            .await
            .is_none()
    );
    delivery.mark_terminal(generation.wrapping_sub(1));
    assert_eq!(
        delivery.failure().retry_advice,
        mini_sub2api_protocol_v1::RetryAdvice::Never
    );
}
