use super::*;

fn message(id: &str, text: &str) -> Value {
    json!({"type":"message","id":id,"role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":text}]})
}

fn partial_events() -> Vec<Value> {
    vec![
        json!({"type":"response.output_item.done","output_index":0,"item":message("msg_first", "finished prefix")}),
        json!({"type":"response.output_item.added","output_index":1,"item":{"type":"message","id":"msg_partial","role":"assistant","status":"in_progress","content":[]}}),
        json!({"type":"response.output_text.delta","output_index":1,"item_id":"msg_partial","content_index":0,"delta":"unfinished suffix"}),
    ]
}

#[tokio::test]
async fn unresolved_output_rejects_completed_and_cannot_become_history() {
    for footer in [
        None,
        Some(json!([])),
        Some(json!([message("msg_first", "finished prefix")])),
    ] {
        let (_temp, store) = store();
        let prepared = prepare(&store, request(json!([input("start")])))
            .await
            .unwrap();
        let context = ResponseStateContext::new(
            OWNER,
            NAMESPACE,
            KEY,
            &store,
            prepared.resolved_identity.as_ref(),
            None,
        )
        .with_operation(prepared.operation);
        let created = context
            .translate_value(json!({"type":"response.created","response":{"id":"resp_partial"}}))
            .await
            .unwrap();
        for event in partial_events() {
            context.translate_value(event).await.unwrap();
        }
        let mut response = json!({"id":"resp_partial","status":"completed"});
        if let Some(output) = footer {
            response["output"] = output;
        }
        let result = context
            .translate_value(json!({"type":"response.completed","response":response}))
            .await;
        let mut next = request(json!([input("continue")]));
        next["previous_response_id"] = created["response"]["id"].clone();
        let reused = prepare(&store, next).await;
        assert!(
            result.is_err(),
            "unfinished suffix was accepted as completed"
        );
        assert!(
            matches!(reused, Err(StatefulPrepareError::StateUnavailable)),
            "partial history was published"
        );
    }
}

#[tokio::test]
async fn finished_items_or_a_complete_footer_preserve_full_history() {
    for finish_with_done in [false, true] {
        let (_temp, store) = store();
        let prepared = prepare(&store, request(json!([input("start")])))
            .await
            .unwrap();
        let context = ResponseStateContext::new(
            OWNER,
            NAMESPACE,
            KEY,
            &store,
            prepared.resolved_identity.as_ref(),
            None,
        )
        .with_operation(prepared.operation);
        context
            .translate_value(json!({"type":"response.created","response":{"id":"resp_complete"}}))
            .await
            .unwrap();
        for event in partial_events() {
            context.translate_value(event).await.unwrap();
        }
        let final_item = message("msg_partial", "unfinished suffix now finished");
        let output = if finish_with_done {
            context
                .translate_value(
                    json!({"type":"response.output_item.done","output_index":1,"item":final_item}),
                )
                .await
                .unwrap();
            json!([])
        } else {
            json!([message("msg_first", "finished prefix"), final_item])
        };
        let result = context.translate_value(json!({"type":"response.completed","response":{"id":"resp_complete","output":output}})).await.unwrap();
        let mut next = request(json!([input("continue")]));
        next["previous_response_id"] = result["response"]["id"].clone();
        let next = prepare(&store, next).await.unwrap();
        let wire: Value = serde_json::from_slice(&next.body).unwrap();
        assert_eq!(wire["input"].as_array().unwrap().len(), 4);
        assert!(wire["input"][2]["content"][0]["text"] == "unfinished suffix now finished");
    }
}

#[tokio::test]
async fn unfinished_items_still_allow_failed_and_incomplete_terminals() {
    for kind in ["response.failed", "response.incomplete"] {
        let (_temp, store) = store();
        let prepared = prepare(&store, request(json!([input("start")])))
            .await
            .unwrap();
        let context = ResponseStateContext::new(
            OWNER,
            NAMESPACE,
            KEY,
            &store,
            prepared.resolved_identity.as_ref(),
            None,
        )
        .with_operation(prepared.operation);
        for event in partial_events() {
            context.translate_value(event).await.unwrap();
        }
        let result = context
            .translate_value(json!({"type":kind,"response":{"id":"resp_failure","output":[]}}))
            .await
            .unwrap();
        let mut next = request(json!([input("continue")]));
        next["previous_response_id"] = result["response"]["id"].clone();
        assert!(matches!(
            prepare(&store, next).await,
            Err(StatefulPrepareError::StateUnavailable)
        ));
    }
}

#[tokio::test]
async fn unfinished_sibling_prevents_compaction_commit() {
    let (_temp, store) = store();
    let mut body = request(json!([{"type":"compaction_trigger"}]));
    body["client_metadata"] = json!({"session_id":"compact","x-codex-turn-metadata":
        json!({"request_kind":"compaction","compaction":{"implementation":"responses_compaction_v2"}}).to_string()});
    let prepared = prepare(&store, body).await.unwrap();
    let identity = prepared.resolved_identity.clone().unwrap();
    let context = ResponseStateContext::new(
        OWNER,
        NAMESPACE,
        KEY,
        &store,
        prepared.resolved_identity.as_ref(),
        prepared.pending_compaction.as_ref(),
    )
    .with_operation(prepared.operation);
    let item = json!({"type":"compaction","encrypted_content":"synthetic checkpoint"});
    context
        .translate_value(json!({"type":"response.output_item.done","output_index":0,"item":item}))
        .await
        .unwrap();
    context.translate_value(json!({"type":"response.output_text.delta","output_index":1,"item_id":"msg_partial","delta":"partial"})).await.unwrap();
    assert!(context.translate_value(json!({"type":"response.completed","response":{"id":"resp_compact_partial","output":[item]}})).await.is_err());
    let window = store
        .edit(NAMESPACE, OWNER, KEY, move |editor| {
            Ok(editor.window_number(&identity.thread_id).unwrap())
        })
        .await
        .unwrap();
    assert_eq!(window, 0);
}
