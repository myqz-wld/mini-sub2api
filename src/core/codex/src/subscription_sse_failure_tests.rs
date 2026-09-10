use super::*;
use crate::response_sse_translation::{UpstreamByteStream, translated_sse_frames};
use futures_util::StreamExt;

fn completed_item() -> Value {
    json!({"id":"msg_failure","type":"message","role":"assistant","content":[{"type":"output_text","text":"finished prefix"}]})
}

fn failed_events() -> Vec<Value> {
    vec![
        json!({"type":"response.created","response":{"id":"resp_failure"}}),
        json!({"type":"response.output_item.done","output_index":0,"item":completed_item()}),
        json!({"type":"error","code":"synthetic","message":"synthetic failure"}),
        json!({"type":"response.failed","response":{"id":"resp_failure","status":"failed","output":[completed_item()],"usage":{"total_tokens":17}}}),
    ]
}

fn upstream(events: &[Value], chunk_size: usize) -> UpstreamByteStream {
    let bytes = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    Box::pin(futures_util::stream::iter(
        bytes
            .as_bytes()
            .chunks(chunk_size)
            .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>(),
    ))
}

fn response_context(
    store: &RequestStateStore,
    prepared: PreparedEmulatedRequest,
) -> ResponseStateContext {
    ResponseStateContext::new(
        OWNER,
        NAMESPACE,
        KEY,
        store,
        prepared.resolved_identity.as_ref(),
        None,
    )
    .with_operation(prepared.operation)
}

fn data_event(frame: http_body::Frame<Bytes>) -> Value {
    let bytes = frame
        .into_data()
        .unwrap_or_else(|_| panic!("expected SSE event, got failure trailers"));
    let text = std::str::from_utf8(&bytes).unwrap();
    serde_json::from_str(text.trim().strip_prefix("data: ").unwrap()).unwrap()
}

#[tokio::test]
async fn sse_error_preserves_the_failed_footer_across_chunk_boundaries() {
    for chunk_size in [1, 7, usize::MAX] {
        let (_temp, store) = store();
        let prepared = prepare(&store, request(json!([input("start")])))
            .await
            .unwrap();
        let context = response_context(&store, prepared);
        let frames = translated_sse_frames(upstream(&failed_events(), chunk_size), context, 4096)
            .collect::<Vec<_>>()
            .await;
        let events: Vec<Value> = frames
            .into_iter()
            .map(|frame| data_event(frame.unwrap()))
            .collect();
        assert_eq!(events.len(), 4);
        assert_eq!(events[2]["type"], "error");
        assert_eq!(events[3]["type"], "response.failed");
        assert_eq!(events[3]["response"]["usage"]["total_tokens"], 17);
        assert_eq!(events[0]["response"]["id"], events[3]["response"]["id"]);
        assert_ne!(events[3]["response"]["id"], "resp_failure");
        let mut next = request(json!([input("continue")]));
        next["previous_response_id"] = events[3]["response"]["id"].clone();
        assert!(matches!(
            prepare(&store, next).await,
            Err(StatefulPrepareError::StateUnavailable)
        ));
        let inner = store.contexts.inner.lock().unwrap();
        assert!(inner.operations.is_empty());
        assert!(inner.reservations.is_empty());
    }
}

#[tokio::test]
async fn sse_failure_tail_rejects_success_output_conflicts_and_duplicate_footers() {
    for case in [
        "completed",
        "incomplete",
        "delta",
        "foreign-id",
        "malformed",
        "changed-item",
        "duplicate",
    ] {
        let (_temp, store) = store();
        let prepared = prepare(&store, request(json!([input("start")])))
            .await
            .unwrap();
        let context = response_context(&store, prepared);
        let mut events = failed_events();
        match case {
            "completed" => events[3]["type"] = "response.completed".into(),
            "incomplete" => events[3]["type"] = "response.incomplete".into(),
            "delta" => {
                events[3] = json!({"type":"response.output_text.delta","delta":"late output"})
            }
            "foreign-id" => events[3]["response"]["id"] = "resp_foreign".into(),
            "malformed" => events[3]["response"]["output"] = json!({}),
            "changed-item" => {
                events[3]["response"]["output"][0]["content"][0]["text"] = "changed".into()
            }
            "duplicate" => events.push(events[3].clone()),
            _ => unreachable!(),
        }
        let stream = translated_sse_frames(upstream(&events, usize::MAX), context, 4096);
        futures_util::pin_mut!(stream);
        let prefix_len = if case == "duplicate" { 4 } else { 3 };
        for _ in 0..prefix_len {
            data_event(stream.next().await.unwrap().unwrap());
        }
        let ledger = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
        let frame = stream.next().await.unwrap().unwrap();
        let trailers = frame
            .trailers_ref()
            .expect("invalid failure footer was forwarded");
        assert_eq!(
            trailers[mini_sub2api_protocol_v1::FAILURE_PHASE_TRAILER],
            "upstream_stream"
        );
        assert!(stream.next().await.is_none());
        assert!(
            std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap() == ledger,
            "rejected footer committed aliases"
        );
        let inner = store.contexts.inner.lock().unwrap();
        assert!(inner.operations.is_empty());
        assert!(inner.reservations.is_empty());
    }
}

#[tokio::test]
async fn sse_error_releases_the_lane_before_its_footer_without_touching_a_retry() {
    let (_temp, store) = store();
    let mut body = request(json!([input("retry")]));
    body["client_metadata"] = json!({"session_id":"session","turn_id":"turn"});
    let first = prepare(&store, body.clone()).await.unwrap();
    let first_id = first.operation.as_ref().unwrap().0.id.clone();
    let context = response_context(&store, first);
    let stream = translated_sse_frames(upstream(&failed_events(), 7), context, 4096);
    futures_util::pin_mut!(stream);
    for _ in 0..2 {
        data_event(stream.next().await.unwrap().unwrap());
    }
    let reserved = store.contexts.inner.lock().unwrap().operations[&first_id].reserved;
    let error = data_event(stream.next().await.unwrap().unwrap());
    assert_eq!(error["type"], "error");
    {
        let inner = store.contexts.inner.lock().unwrap();
        assert!(inner.operations.is_empty());
        assert_eq!(inner.reservations[&first_id].reserved, reserved);
        assert!(inner.scopes.values().all(|scope| scope.records.is_empty()));
    }
    let retry = prepare(&store, body).await.unwrap();
    let retry_id = retry.operation.as_ref().unwrap().0.id.clone();
    assert_eq!(
        data_event(stream.next().await.unwrap().unwrap())["type"],
        "response.failed"
    );
    assert!(stream.next().await.is_none());
    let inner = store.contexts.inner.lock().unwrap();
    assert!(inner.operations.contains_key(&retry_id));
    assert!(inner.reservations.is_empty());
}

#[tokio::test]
async fn sse_error_only_releases_validation_budget_on_eof_or_cancellation() {
    for eof in [false, true] {
        let (_temp, store) = store();
        let prepared = prepare(&store, request(json!([input("start")])))
            .await
            .unwrap();
        let context = response_context(&store, prepared);
        let events = vec![json!({"type":"error","code":"synthetic"})];
        let mut stream = Box::pin(translated_sse_frames(upstream(&events, 7), context, 4096));
        assert_eq!(
            data_event(stream.next().await.unwrap().unwrap())["type"],
            "error"
        );
        {
            let inner = store.contexts.inner.lock().unwrap();
            assert!(inner.operations.is_empty());
            assert_eq!(inner.reservations.len(), 1);
        }
        if eof {
            assert!(stream.next().await.is_none());
        }
        drop(stream);
        assert!(store.contexts.inner.lock().unwrap().reservations.is_empty());
    }
}

#[tokio::test]
async fn sse_failed_footer_keeps_reasoning_visibility_and_unfinished_output_failure() {
    let (_temp, store) = store();
    let mut body = request(json!([input("start")]));
    body["include"] = json!([]);
    let prepared = prepare(&store, body).await.unwrap();
    let context = response_context(&store, prepared);
    let reasoning = json!({"id":"rs_failure","type":"reasoning","summary":[],"encrypted_content":"synthetic-hidden"});
    let events = vec![
        json!({"type":"response.output_item.done","output_index":0,"item":reasoning}),
        json!({"type":"response.output_text.delta","output_index":1,"item_id":"msg_unfinished","delta":"partial"}),
        json!({"type":"error","code":"synthetic"}),
        json!({"type":"response.failed","response":{"id":"resp_failure","output":[reasoning],"usage":{"total_tokens":17}}}),
    ];
    let frames = translated_sse_frames(upstream(&events, 7), context, 4096)
        .collect::<Vec<_>>()
        .await;
    let events: Vec<Value> = frames
        .into_iter()
        .map(|frame| data_event(frame.unwrap()))
        .collect();
    assert_eq!(events.len(), 4);
    assert!(events[0]["item"].get("encrypted_content").is_none());
    assert!(
        events[3]["response"]["output"][0]
            .get("encrypted_content")
            .is_none()
    );
    assert_eq!(events[3]["response"]["usage"]["total_tokens"], 17);
}
