use super::*;
use crate::request_state_types::WireIdDomain;
use crate::request_wire_ids::translate_request_ids;
use std::collections::BTreeSet;

#[tokio::test]
async fn stream_eof_requires_a_terminal_and_preserves_the_last_event() {
    for (data, terminal) in [
        ("", false),
        (": keepalive\n\ndata: [DONE]\n\n", false),
        (
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_eof\"}}\n\n",
            false,
        ),
        (
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}",
            false,
        ),
        (
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_eof\",\"output\":[]}}",
            true,
        ),
        (
            "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_eof\"}}\r\n\r\n",
            true,
        ),
        (
            "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_eof\"}}\n\n",
            true,
        ),
        (
            "data: {\"type\":\"error\",\"error\":{\"code\":\"synthetic\"}}\n\n",
            true,
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = crate::request_state_store::RequestStateStore::new(temp.path().to_path_buf());
        let context =
            ResponseStateContext::new("acct_eof", "namespace-eof", "scope-eof", &store, None, None);
        let upstream: UpstreamByteStream = Box::pin(futures_util::stream::iter(
            data.as_bytes()
                .chunks(7)
                .map(|v| Ok(Bytes::copy_from_slice(v)))
                .collect::<Vec<_>>(),
        ));
        let frames = translated_sse_frames(upstream, context, 1024 * 1024)
            .collect::<Vec<_>>()
            .await;
        let mut payload = Vec::new();
        let mut failures = 0;
        for frame in frames {
            let frame = frame.unwrap();
            if let Some(bytes) = frame.data_ref() {
                payload.extend_from_slice(bytes);
            }
            if let Some(trailers) = frame.trailers_ref() {
                failures += 1;
                assert_eq!(trailers[FAILURE_PHASE_TRAILER], "upstream_stream");
                assert_eq!(trailers[DELIVERY_STATE_TRAILER], "delivered");
                assert_eq!(trailers[RETRY_ADVICE_TRAILER], "never");
            }
        }
        assert_eq!(failures, usize::from(!terminal), "EOF terminal evidence");
        assert_eq!(
            payload.is_empty(),
            data.is_empty(),
            "last event was discarded"
        );
    }
}

#[test]
fn detects_lf_and_crlf_event_boundaries() {
    assert_eq!(find_event_end(b"data: {}\n\nnext"), Some(10));
    assert_eq!(find_event_end(b"data: {}\r\n\r\nnext"), Some(12));
    assert_eq!(find_event_end(b"data: {}\n"), None);
}

#[test]
fn replaces_multiline_data_and_preserves_event_fields() {
    let event = "event: response.completed\r\nid: 7\r\ndata: {\"type\":\r\ndata: \"response.completed\"}\r\n\r\n";
    assert_eq!(
        data_payload(event).as_deref(),
        Some("{\"type\":\n\"response.completed\"}")
    );
    let got = replace_data_lines(event, "{\"type\":\"response.completed\"}").expect("replace data");
    assert!(got.contains("event: response.completed\r\n"));
    assert!(got.contains("id: 7\r\n"));
    assert_eq!(got.matches("data:").count(), 1);
}

#[tokio::test]
async fn split_sse_event_persists_aliases_before_delivery_and_round_trips() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = crate::request_state_store::RequestStateStore::new(temp.path().to_path_buf());
    store
        .edit(
            "namespace-sse",
            "acct_sse_translation",
            "scope-sse",
            |editor| {
                editor.bind_wire_pair(WireIdDomain::Turn, "turn_downstream", "turn_upstream")?;
                Ok(())
            },
        )
        .await
        .expect("seed mappings");
    let context = ResponseStateContext::new(
        "acct_sse_translation",
        "namespace-sse",
        "scope-sse",
        &store,
        None,
        None,
    );
    let event = concat!(
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_upstream\",",
        "\"output\":[{\"type\":\"function_call\",\"id\":\"item_provider\",",
        "\"call_id\":\"call_provider\",\"arguments\":\"{\\\"opaque_id\\\":\\\"keep\\\"}\",",
        "\"internal_chat_message_metadata_passthrough\":{\"turn_id\":\"turn_upstream\"}}]}}\n\n"
    );
    let split = event.len() / 2;
    let upstream: UpstreamByteStream = Box::pin(futures_util::stream::iter(vec![
        Ok(Bytes::copy_from_slice(&event.as_bytes()[..split])),
        Ok(Bytes::copy_from_slice(&event.as_bytes()[split..])),
    ]));
    let frames = translated_sse_frames(upstream, context, 1024 * 1024)
        .collect::<Vec<_>>()
        .await;
    assert_eq!(frames.len(), 1);
    let bytes = frames
        .into_iter()
        .next()
        .expect("frame")
        .expect("infallible")
        .into_data()
        .expect("data frame");
    assert!(store.state_path_for_test("namespace-sse").is_file());
    let text = std::str::from_utf8(&bytes).expect("translated SSE");
    let payload = data_payload(text).expect("SSE data");
    let value: serde_json::Value = serde_json::from_str(&payload).expect("event JSON");
    let response_alias = value["response"]["id"]
        .as_str()
        .expect("response alias")
        .to_string();
    assert_ne!(response_alias, "resp_upstream");
    assert_eq!(
        value["response"]["output"][0]["internal_chat_message_metadata_passthrough"]["turn_id"],
        "turn_downstream"
    );
    let item_alias = value["response"]["output"][0]["id"]
        .as_str()
        .expect("item alias")
        .to_string();
    let call_alias = value["response"]["output"][0]["call_id"]
        .as_str()
        .expect("call alias")
        .to_string();
    assert_ne!(item_alias, "item_provider");
    assert_ne!(call_alias, "call_provider");

    let restored = store
        .edit(
            "namespace-sse",
            "acct_sse_translation",
            "scope-sse",
            move |editor| {
                let mut request = serde_json::json!({
                    "previous_response_id":response_alias,
                    "input":[{"type":"function_call_output","id":item_alias,"call_id":call_alias,"output":"ok"}]
                })
                .as_object()
                .expect("request")
                .clone();
                translate_request_ids(editor, &mut request, &BTreeSet::new())?;
                Ok(request)
            },
        )
        .await
        .expect("restore aliases");
    assert_eq!(restored["previous_response_id"], "resp_upstream");
    assert_eq!(restored["input"][0]["id"], "item_provider");
    assert_eq!(restored["input"][0]["call_id"], "call_provider");
}

#[tokio::test]
async fn first_translation_failure_reports_upstream_response_as_delivered() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = crate::request_state_store::RequestStateStore::new(temp.path().to_path_buf());
    store
        .edit(
            "namespace-sse-failure",
            "acct_sse_failure",
            "scope-sse-failure",
            |editor| {
                editor.bind_wire_pair(
                    WireIdDomain::Response,
                    "resp_downstream_seed",
                    "resp_upstream_seed",
                )?;
                Ok(())
            },
        )
        .await
        .expect("seed request state");
    std::fs::write(
        store.state_path_for_test("namespace-sse-failure"),
        b"{corrupt",
    )
    .expect("corrupt request state");
    let context = ResponseStateContext::new(
        "acct_sse_failure",
        "namespace-sse-failure",
        "scope-sse-failure",
        &store,
        None,
        None,
    );
    let upstream: UpstreamByteStream = Box::pin(futures_util::stream::iter(vec![Ok(
        Bytes::from_static(
            b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_provider\"}}\n\n",
        ),
    )]));
    let frames = translated_sse_frames(upstream, context, 1024 * 1024)
        .collect::<Vec<_>>()
        .await;
    assert_eq!(frames.len(), 1);
    let trailers = frames
        .into_iter()
        .next()
        .expect("failure frame")
        .expect("infallible")
        .into_trailers()
        .expect("failure trailers");
    assert_eq!(trailers[FAILURE_PHASE_TRAILER], "upstream_stream");
    assert_eq!(trailers[DELIVERY_STATE_TRAILER], "delivered");
    assert_eq!(trailers[RETRY_ADVICE_TRAILER], "never");
}

#[cfg(unix)]
#[tokio::test]
async fn completed_compaction_write_failure_emits_delivered_and_keeps_window_pending() {
    use crate::request_compaction::PendingCompaction;
    use crate::request_state_types::PersistedRequestState;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let store = crate::request_state_store::RequestStateStore::new(temp.path().to_path_buf());
    let pending = store
        .edit(
            "namespace-compaction-write",
            "acct_compaction_write",
            "scope-compaction-write",
            |editor| {
                let conversation_key = editor.lookup("conversation", "write-session");
                let marker_key = editor.lookup("compaction", "write-operation");
                let conversation = editor.conversation(&conversation_key)?;
                let target = editor.begin_compaction(&marker_key, &conversation.id)?;
                Ok(PendingCompaction {
                    marker_key,
                    thread_id: conversation.id,
                    target_window: target,
                    requires_compaction_item: true,
                })
            },
        )
        .await
        .expect("pending compaction");
    let context = ResponseStateContext::new(
        "acct_compaction_write",
        "namespace-compaction-write",
        "scope-compaction-write",
        &store,
        None,
        Some(&pending),
    );
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o500))
        .expect("make state directory read-only");
    let upstream: UpstreamByteStream = Box::pin(futures_util::stream::iter(vec![Ok(
        Bytes::from_static(
            b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_write_failure\",\"output\":[{\"type\":\"compaction\",\"encrypted_content\":\"synthetic\"}]}}\n\n",
        ),
    )]));
    let frames = translated_sse_frames(upstream, context, 1024 * 1024)
        .collect::<Vec<_>>()
        .await;
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))
        .expect("restore state directory permissions");
    let trailers = frames
        .into_iter()
        .next()
        .expect("failure frame")
        .expect("infallible")
        .into_trailers()
        .expect("failure trailers");
    assert_eq!(trailers[DELIVERY_STATE_TRAILER], "delivered");
    assert_eq!(trailers[RETRY_ADVICE_TRAILER], "never");

    let state: PersistedRequestState = serde_json::from_slice(
        &std::fs::read(store.state_path_for_test("namespace-compaction-write"))
            .expect("pending state"),
    )
    .expect("pending state JSON");
    let scope = state.scopes.values().next().expect("scope");
    assert_eq!(
        scope.conversations.values().next().unwrap().window_number,
        0
    );
    assert_eq!(
        scope
            .compaction_markers
            .values()
            .next()
            .unwrap()
            .window_number,
        1
    );
}
