use super::*;
use crate::request_state_store::RequestStateStore;
use crate::request_state_types::WireIdDomain;

#[tokio::test]
#[ignore = "manual resource benchmark; run in release mode"]
async fn benchmark_websocket_large_delta_frames() {
    let temporary = tempfile::tempdir().unwrap();
    let store = RequestStateStore::new(temporary.path().into());
    store
        .edit(
            "synthetic-ws-benchmark",
            "acct_benchmark",
            "synthetic-key",
            |editor| {
                editor.bind_wire_pair(
                    WireIdDomain::Item,
                    "item_public_benchmark",
                    "item_provider_benchmark",
                )
            },
        )
        .await
        .unwrap();
    crate::response_state_stamp::StateStamp::wait_until_cacheable(
        &store.state_path_for_test("synthetic-ws-benchmark"),
    );
    let state = ResponseStateContext::new(
        "acct_benchmark",
        "synthetic-ws-benchmark",
        "synthetic-key",
        &store,
        None,
        None,
    );
    let value = serde_json::json!({"type":"response.output_text.delta","item_id":"item_provider_benchmark",
        "delta":"synthetic content ".repeat(32 * 1024),"output_index":0,"content_index":0});
    let text = value.to_string();
    for subscription in [false, true] {
        let context = subscription.then_some(&state);
        let continuation = Arc::new(StdMutex::new(ResponsesWebSocketState::new(
            CallerKind::Codex,
            UpstreamProfile::CodexSubscription1580,
        )));
        let delivery = WebSocketDeliveryTracker::default();
        let mut inbound = Inbound::default();
        inbound
            .translate(text.clone(), context, &continuation, &delivery)
            .await
            .unwrap();
        for round in 0..3 {
            let started = std::time::Instant::now();
            for _ in 0..64 {
                let (output, terminal) = inbound
                    .translate(text.clone(), context, &continuation, &delivery)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(terminal.is_none());
                if !subscription {
                    assert_eq!(output, text);
                } else {
                    assert!(output.contains("item_public_benchmark"));
                    assert!(!output.contains("item_provider_benchmark"));
                    assert!(output.len() >= value["delta"].as_str().unwrap().len());
                }
            }
            println!(
                "websocket_delta_benchmark subscription={subscription} frame_bytes={} round={round} events=64 elapsed_ms={:.3}",
                text.len(),
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}
