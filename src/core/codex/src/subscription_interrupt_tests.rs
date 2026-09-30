use super::repair_tests::websocket_request;
use super::*;

fn lite(items: Value) -> Value {
    let mut value = request(items);
    value["model"] = "gpt-6-sol".into();
    value
}

fn context(store: &RequestStateStore, prepared: &PreparedEmulatedRequest) -> ResponseStateContext {
    ResponseStateContext::new(
        OWNER,
        NAMESPACE,
        KEY,
        store,
        prepared.resolved_identity.as_ref(),
        None,
    )
    .with_operation(prepared.operation.clone())
}

fn message(id: &str, text: &str) -> Value {
    json!({"type":"message","id":id,"role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":text}]})
}

fn interrupt(id: &Value) -> Value {
    json!({"type":"response.interrupt","response_id":id,"mode":"discard_partial_items"})
}

#[tokio::test]
async fn interrupted_invalid_output_never_publishes_history_or_dependencies() {
    let cases = crate::response_interrupt::fixtures::inconsistent_outputs("resp_integrity")
        .into_iter()
        .flat_map(|(name, events)| {
            [None, Some(json!([]))].map(|footer| (name.clone(), events.clone(), true, footer))
        })
        .chain(
            crate::response_interrupt::fixtures::inconsistent_terminals("resp_integrity")
                .into_iter()
                .map(|(name, events, footer)| (name, events, false, footer)),
        );
    for (name, events, early_failure, footer) in cases {
        let (_temp, store) = store();
        let socket = store.contexts.open_socket().unwrap();
        let first = websocket_request(&store, lite(json!([input("initial")])), &socket.id, None)
            .await
            .unwrap();
        let response = context(&store, &first);
        let created = response
            .translate_value(json!({"type":"response.created",
                "response":{"id":"resp_integrity"}}))
            .await
            .unwrap();
        store
            .contexts
            .prepare_control(
                &ContextStore::scope_key(NAMESPACE, KEY),
                first.resolved_identity.as_ref(),
                &interrupt(&created["response"]["id"]),
            )
            .unwrap();
        for (index, event) in events.iter().enumerate() {
            let result = response.translate_value(event.clone()).await;
            assert_eq!(
                result.is_err(),
                early_failure && index + 1 == events.len(),
                "{name}: {index}"
            );
        }
        let mut terminal = json!({"type":"response.incomplete","response":{
                "id":"resp_integrity","status":"incomplete","incomplete_details":{"reason":"interrupted"}}});
        if let Some(output) = footer {
            terminal["response"]["output"] = output;
        }
        assert!(response.translate_value(terminal).await.is_err(), "{name}");
        let mut next = lite(json!([input("continue")]));
        next["previous_response_id"] = created["response"]["id"].clone();
        assert!(prepare(&store, next).await.is_err(), "{name}");
    }
}

#[tokio::test]
async fn interrupted_history_retains_only_done_items_across_index_gaps_and_transport_change() {
    for footer in [false, true] {
        let (_temp, store) = store();
        let socket = store.contexts.open_socket().unwrap();
        let first = websocket_request(&store, lite(json!([input("initial")])), &socket.id, None)
            .await
            .unwrap();
        let response = context(&store, &first);
        let created = response
            .translate_value(json!({"type":"response.created","response":{"id":"resp_interrupt"}}))
            .await
            .unwrap();
        let public_id = created["response"]["id"].clone();
        store
            .contexts
            .prepare_control(
                &ContextStore::scope_key(NAMESPACE, KEY),
                first.resolved_identity.as_ref(),
                &interrupt(&public_id),
            )
            .unwrap();
        response
            .translate_value(
                json!({"type":"response.interrupt.accepted","response_id":"resp_interrupt"}),
            )
            .await
            .unwrap();
        let first_item = message("msg_done_first", "kept first");
        let last_item = message("msg_done_last", "kept last");
        response
            .translate_value(
                json!({"type":"response.output_item.done","output_index":0,"item":first_item}),
            )
            .await
            .unwrap();
        response.translate_value(json!({"type":"response.output_item.added","output_index":1,"item":{
            "type":"message","id":"msg_discarded","role":"assistant","status":"in_progress","content":[]}})).await.unwrap();
        response.translate_value(json!({"type":"response.output_text.delta","output_index":1,"item_id":"msg_discarded","delta":"discarded partial payload"})).await.unwrap();
        response.translate_value(json!({"type":"response.output_item.interrupted","response_id":"resp_interrupt","output_index":1,"item_id":"msg_discarded"})).await.unwrap();
        response
            .translate_value(
                json!({"type":"response.output_item.done","output_index":2,"item":last_item}),
            )
            .await
            .unwrap();
        let output = if footer {
            json!([first_item, last_item])
        } else {
            json!([])
        };
        let terminal = response.translate_value(json!({"type":"response.incomplete","response":{
            "id":"resp_interrupt","status":"incomplete","incomplete_details":{"reason":"interrupted"},"output":output}})).await.unwrap();
        assert_eq!(terminal["type"], "response.incomplete");
        assert_eq!(
            terminal["response"]["incomplete_details"]["reason"],
            "interrupted"
        );
        let mut next = lite(json!([input("continue")]));
        next["previous_response_id"] = public_id.clone();
        let next = prepare(&store, next).await.unwrap();
        let wire: Value = serde_json::from_slice(&next.body).unwrap();
        let text = wire.to_string();
        assert!(text.contains("kept first") && text.contains("kept last"));
        assert!(!text.contains("discarded partial payload"));
        let assistant_count = wire["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["role"] == "assistant")
            .count();
        assert_eq!(assistant_count, 2);
        drop(next);
        let mut delta = lite(json!([input("socket suffix")]));
        delta["previous_response_id"] = public_id;
        let next = websocket_request(&store, delta, &socket.id, first.resolved_identity.as_ref())
            .await
            .unwrap();
        let wire: Value = serde_json::from_slice(&next.body).unwrap();
        assert_eq!(wire["previous_response_id"], "resp_interrupt");
    }
}

#[tokio::test]
async fn interruption_is_one_shot_and_requires_the_active_lite_scope_and_socket() {
    let (_temp, store) = store();
    let socket = store.contexts.open_socket().unwrap();
    let first = websocket_request(&store, lite(json!([input("initial")])), &socket.id, None)
        .await
        .unwrap();
    let response = context(&store, &first);
    let scope = ContextStore::scope_key(NAMESPACE, KEY);
    assert!(
        store
            .contexts
            .prepare_control(
                &scope,
                first.resolved_identity.as_ref(),
                &interrupt(&json!("not-created"))
            )
            .is_err()
    );
    let created = response
        .translate_value(json!({"type":"response.created","response":{"id":"resp_owner"}}))
        .await
        .unwrap();
    let control = interrupt(&created["response"]["id"]);
    for changed in ["mode", "response", "extra", "scope", "socket", "thread"] {
        let mut invalid = control.clone();
        let mut identity = first.resolved_identity.clone().unwrap();
        let mut candidate_scope = scope.clone();
        match changed {
            "mode" => invalid["mode"] = "unsupported".into(),
            "response" => invalid["response_id"] = "other-response".into(),
            "extra" => invalid["input"] = json!([]),
            "scope" => candidate_scope = "other-key".into(),
            "socket" => identity.connection_id = Some("other-socket".into()),
            "thread" => identity.thread_id = "other-thread".into(),
            _ => unreachable!(),
        }
        assert!(
            store
                .contexts
                .prepare_control(&candidate_scope, Some(&identity), &invalid)
                .is_err(),
            "{changed}"
        );
    }
    store
        .contexts
        .prepare_control(&scope, first.resolved_identity.as_ref(), &control)
        .unwrap();
    assert!(
        store
            .contexts
            .prepare_control(&scope, first.resolved_identity.as_ref(), &control)
            .is_err()
    );
    response
        .translate_value(
            json!({"type":"response.completed","response":{"id":"resp_owner","output":[]}}),
        )
        .await
        .unwrap();
    assert!(
        store
            .contexts
            .prepare_control(&scope, first.resolved_identity.as_ref(), &control)
            .is_err()
    );

    for (model, generate) in [("gpt-5.5", true), ("gpt-6-sol", false)] {
        let socket = store.contexts.open_socket().unwrap();
        let body = json!({"model":model,"generate":generate,"input":[]});
        let first = websocket_request(&store, body, &socket.id, None)
            .await
            .unwrap();
        let created = context(&store, &first).translate_value(json!({"type":"response.created","response":{"id":format!("resp_{model}_{generate}")}})).await.unwrap();
        assert!(
            store
                .contexts
                .prepare_control(
                    &scope,
                    first.resolved_identity.as_ref(),
                    &interrupt(&created["response"]["id"])
                )
                .is_err()
        );
    }
}

#[tokio::test]
async fn unsolicited_or_other_incomplete_reasons_cannot_become_baselines() {
    for (requested, reason) in [
        (false, "interrupted"),
        (true, "content_filter"),
        (true, "unknown"),
    ] {
        let (_temp, store) = store();
        let socket = store.contexts.open_socket().unwrap();
        let first = websocket_request(&store, lite(json!([input("initial")])), &socket.id, None)
            .await
            .unwrap();
        let response = context(&store, &first);
        let created = response
            .translate_value(json!({"type":"response.created","response":{"id":"resp_incomplete"}}))
            .await
            .unwrap();
        if requested {
            store
                .contexts
                .prepare_control(
                    &ContextStore::scope_key(NAMESPACE, KEY),
                    first.resolved_identity.as_ref(),
                    &interrupt(&created["response"]["id"]),
                )
                .unwrap();
        }
        let terminal = response.translate_value(json!({"type":"response.incomplete","response":{"id":"resp_incomplete","incomplete_details":{"reason":reason},"output":[]}})).await.unwrap();
        let mut next = lite(json!([input("continue")]));
        next["previous_response_id"] = terminal["response"]["id"].clone();
        assert!(matches!(
            prepare(&store, next).await,
            Err(StatefulPrepareError::StateUnavailable)
        ));
    }
}

#[tokio::test]
async fn interrupted_footer_cannot_complete_unproven_or_discarded_output() {
    for attack in [
        "unfinished",
        "resurrected",
        "extra-footer",
        "wrong-response",
    ] {
        let (_temp, store) = store();
        let socket = store.contexts.open_socket().unwrap();
        let first = websocket_request(&store, lite(json!([input("initial")])), &socket.id, None)
            .await
            .unwrap();
        let response = context(&store, &first);
        let created = response
            .translate_value(json!({"type":"response.created","response":{"id":"resp_proof"}}))
            .await
            .unwrap();
        store
            .contexts
            .prepare_control(
                &ContextStore::scope_key(NAMESPACE, KEY),
                first.resolved_identity.as_ref(),
                &interrupt(&created["response"]["id"]),
            )
            .unwrap();
        response.translate_value(json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg_partial","type":"message","status":"in_progress"}})).await.unwrap();
        if attack != "unfinished" {
            response.translate_value(json!({"type":"response.output_item.interrupted","response_id":"resp_proof","output_index":0,"item_id":"msg_partial"})).await.unwrap();
        }
        let id = if attack == "wrong-response" {
            "other-response"
        } else {
            "resp_proof"
        };
        let output = match attack {
            "resurrected" => json!([message("msg_partial", "forged completion")]),
            "extra-footer" => json!([message("msg_unknown", "unproven completion")]),
            _ => json!([]),
        };
        assert!(response.translate_value(json!({"type":"response.incomplete","response":{"id":id,"status":"incomplete","incomplete_details":{"reason":"interrupted"},"output":output}})).await.is_err(),"{attack}");
        let mut next = lite(json!([]));
        next["previous_response_id"] = created["response"]["id"].clone();
        assert!(prepare(&store, next).await.is_err());
    }
}
