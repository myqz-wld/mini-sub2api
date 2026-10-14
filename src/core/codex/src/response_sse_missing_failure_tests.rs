use super::*;
use crate::response_sse_reader::SseTimeouts;
use serde_json::{Value, json};
use std::time::Duration;

#[tokio::test]
async fn synthesized_failure_keeps_nested_then_top_level_retry_after() {
    for (nested, top, expected) in [
        (json!({}), json!({"Retry-After":30}), Some("30")),
        (
            json!({"Retry-After":"invalid"}),
            json!({"Retry-After":"30"}),
            Some("30"),
        ),
        (
            json!({"Retry-After":"7"}),
            json!({"Retry-After":"30"}),
            Some("7"),
        ),
        (
            json!({}),
            json!({"Retry-After":"30\r\nAuthorization: private-synthetic"}),
            None,
        ),
    ] {
        for open_tail in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let store = crate::request_state_store::RequestStateStore::new(temp.path().into());
            let context =
                ResponseStateContext::new("acct_retry", "namespace", "scope", &store, None, None);
            let mut headers = top.clone();
            headers["Authorization"] = "private-synthetic".into();
            let event = json!({"type":"error","headers":headers,"error":{
                "code":"server_is_overloaded","message":"private-synthetic",
                "headers":nested}});
            let prefix =
                futures_util::stream::iter([Ok(Bytes::from(format!("data: {event}\n\n")))]);
            let upstream: UpstreamByteStream = if open_tail {
                Box::pin(prefix.chain(futures_util::stream::pending()))
            } else {
                Box::pin(prefix)
            };
            let reader = SseReader::with_timeouts(
                upstream,
                4096,
                SseTimeouts {
                    idle: Duration::from_millis(100),
                    output_idle: Duration::from_millis(100),
                    terminal_tail: Duration::from_millis(50),
                },
            );
            let frames = tokio::time::timeout(
                Duration::from_secs(2),
                translate_reader(reader, context, 4096).collect::<Vec<_>>(),
            )
            .await
            .expect("known failure must terminate");
            assert_eq!(frames.len(), 2);
            let bytes = frames[1].as_ref().unwrap().data_ref().unwrap();
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(!text.contains("private-synthetic"));
            let event: Value = serde_json::from_str(&data_payload(text).unwrap()).unwrap();
            assert_eq!(event["type"], "response.failed");
            assert_eq!(event["response"]["error"]["code"], "server_is_overloaded");
            assert_eq!(
                event
                    .pointer("/response/error/headers/retry-after")
                    .and_then(Value::as_str),
                expected,
                "nested={nested} top={top} open_tail={open_tail}"
            );
            assert!(
                event["response"]["error"]["headers"]
                    .get("Authorization")
                    .is_none()
            );
            assert!(event["response"].get("usage").is_none());
        }
    }
}

#[tokio::test]
async fn error_only_gets_one_private_safe_footer_at_eof_or_read_deadline() {
    for nested in [false, true] {
        for open_tail in [false, true] {
            for chunk_size in [1, 4096] {
                let temp = tempfile::tempdir().unwrap();
                let store = crate::request_state_store::RequestStateStore::new(temp.path().into());
                let context = ResponseStateContext::new(
                    "acct_failure",
                    "namespace",
                    "scope",
                    &store,
                    None,
                    None,
                );
                let error = json!({"code":"context_length_exceeded", "message":"private-synthetic",
                    "headers":{"Retry-After":"2", "Authorization":"private-synthetic"}});
                let mut event = if nested {
                    json!({"type":"error", "error":error})
                } else {
                    let mut event = error;
                    event["type"] = "error".into();
                    event
                };
                event["response_id"] = "resp_private".into();
                let wire = format!(
                    "data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_private\"}}}}\n\ndata: {event}\n\ndata: [DONE]\n\n"
                );
                let prefix = futures_util::stream::iter(
                    wire.as_bytes()
                        .chunks(chunk_size)
                        .map(|part| Ok(Bytes::copy_from_slice(part)))
                        .collect::<Vec<_>>(),
                );
                let upstream: UpstreamByteStream = if open_tail {
                    Box::pin(prefix.chain(futures_util::stream::pending()))
                } else {
                    Box::pin(prefix)
                };
                let reader = SseReader::with_timeouts(
                    upstream,
                    4096,
                    SseTimeouts {
                        idle: Duration::from_millis(200),
                        output_idle: Duration::from_millis(200),
                        terminal_tail: Duration::from_millis(50),
                    },
                );
                // Translation performs real file I/O; keep the runtime clock running.
                let frames = tokio::time::timeout(
                    Duration::from_secs(2),
                    translate_reader(reader, context, 4096).collect::<Vec<_>>(),
                )
                .await
                .expect("known failure must finish at the reader's existing deadline");
                let mut events = Vec::new();
                for frame in frames {
                    let frame = frame.unwrap();
                    assert!(
                        frame.trailers_ref().is_none(),
                        "nested={nested} open_tail={open_tail} chunk_size={chunk_size} delivered={}",
                        events.len()
                    );
                    let bytes = frame.data_ref().unwrap();
                    let text = std::str::from_utf8(bytes).unwrap();
                    assert!(!text.contains("private-synthetic"));
                    assert!(!text.contains("[DONE]"));
                    if let Some(data) = data_payload(text) {
                        events.push(serde_json::from_str::<Value>(&data).unwrap());
                    }
                }
                assert_eq!(events.len(), 3);
                assert_eq!(events[1]["type"], "error");
                let failed = &events[2];
                assert_eq!(failed["type"], "response.failed");
                assert_eq!(
                    failed["response"]["error"]["code"],
                    "context_length_exceeded"
                );
                assert_eq!(
                    failed["response"]["error"]["headers"],
                    json!({"retry-after":"2"})
                );
                assert_eq!(failed["response"]["id"], events[0]["response"]["id"]);
                assert_ne!(failed["response"]["id"], "resp_private");
                assert!(failed["response"].get("output").is_none());
                assert!(failed["response"].get("usage").is_none());
            }
        }
    }
}

#[tokio::test]
async fn delayed_real_failed_footer_keeps_usage_and_is_not_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let store = crate::request_state_store::RequestStateStore::new(temp.path().into());
    let context =
        ResponseStateContext::new("acct_delayed", "namespace", "scope", &store, None, None);
    let prefix = futures_util::stream::once(async {
        Ok(Bytes::from_static(
            b"data: {\"type\":\"error\",\"code\":\"context_length_exceeded\"}\n\n",
        ))
    });
    let footer = futures_util::stream::once(async {
        // Exceed the terminal grace below while staying within the ordinary read deadline.
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(Bytes::from_static(b"data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_delayed\",\"error\":{\"code\":\"context_length_exceeded\"},\"usage\":{\"total_tokens\":17}}}\n\n"))
    });
    let reader = SseReader::with_timeouts(
        Box::pin(prefix.chain(footer)),
        4096,
        SseTimeouts {
            idle: Duration::from_secs(2),
            output_idle: Duration::from_secs(2),
            terminal_tail: Duration::from_millis(50),
        },
    );
    let frames = translate_reader(reader, context, 4096)
        .collect::<Vec<_>>()
        .await;
    assert_eq!(frames.len(), 2);
    let bytes = frames[1]
        .as_ref()
        .unwrap()
        .data_ref()
        .expect("real failed footer");
    let payload = data_payload(std::str::from_utf8(bytes).unwrap()).unwrap();
    let event: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(event["type"], "response.failed");
    assert_eq!(event["response"]["usage"]["total_tokens"], 17);
    assert!(event["response"]["id"].is_string());
}

#[tokio::test]
async fn synthesized_terminal_respects_output_limit_and_does_not_mask_malformed_tail() {
    for tail in ["", "data: invalid\n\n"] {
        let temp = tempfile::tempdir().unwrap();
        let store = crate::request_state_store::RequestStateStore::new(temp.path().into());
        let context =
            ResponseStateContext::new("acct_bounded", "namespace", "scope", &store, None, None);
        let wire = format!("data: {{\"type\":\"error\",\"code\":\"server_error\"}}\n\n{tail}");
        let upstream = Box::pin(futures_util::stream::once(async { Ok(Bytes::from(wire)) }));
        let maximum = if tail.is_empty() { 110 } else { 4096 };
        let frames = translated_sse_frames(upstream, context, maximum)
            .collect::<Vec<_>>()
            .await;
        assert_eq!(frames.len(), 2);
        assert!(frames[0].as_ref().unwrap().data_ref().is_some());
        assert!(frames[1].as_ref().unwrap().trailers_ref().is_some());
    }
}
