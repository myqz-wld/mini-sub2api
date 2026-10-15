use super::*;

async fn complete(
    store: &RequestStateStore,
    prepared: PreparedEmulatedRequest,
    id: &str,
    output: Value,
) -> Value {
    let response = ResponseStateContext::new(
        OWNER,
        NAMESPACE,
        KEY,
        store,
        prepared.resolved_identity.as_ref(),
        prepared.pending_compaction.as_ref(),
    )
    .with_operation(prepared.operation);
    response
        .translate_value(json!({"type":"response.created","response":{"id":id}}))
        .await
        .unwrap();
    for (index, item) in output.as_array().unwrap().iter().enumerate() {
        response
            .translate_value(
                json!({"type":"response.output_item.done","output_index":index,"item":item}),
            )
            .await
            .unwrap();
    }
    response
        .translate_value(json!({"type":"response.completed","response":{"id":id,"output":output}}))
        .await
        .unwrap()["response"]
        .clone()
}

#[tokio::test]
async fn explicit_first_http_requests_fork_without_reassigning_session_or_turn() {
    let (_temp, store) = store();
    let mut body = request(json!([input("synthetic first task")]));
    body["client_metadata"] = json!({"session_id":"shared-session","turn_id":"shared-turn"});
    let first = prepare(&store, body.clone()).await.unwrap();
    body["input"] = json!([input("synthetic second task")]);
    let second = prepare(&store, body).await.unwrap();
    assert_eq!(
        active(&store, &first).identity,
        active(&store, &second).identity
    );
    assert_ne!(
        active(&store, &first).branch,
        active(&store, &second).branch
    );
    let second = complete(
        &store,
        second,
        "resp_explicit_second",
        text_output("second answer"),
    )
    .await;
    let first = complete(
        &store,
        first,
        "resp_explicit_first",
        text_output("first answer"),
    )
    .await;
    for (response, expected) in [
        (first, "synthetic first task"),
        (second, "synthetic second task"),
    ] {
        let mut next = request(json!([input("follow-up")]));
        next["previous_response_id"] = response["id"].clone();
        let next = prepare(&store, next).await.unwrap();
        let history = active(&store, &next).history.unwrap().values();
        assert_eq!(history[0]["content"][0]["text"], expected);
    }
}

#[tokio::test]
async fn independent_connections_and_transports_can_fork_the_same_turn() {
    for (first_ws, second_ws) in [(true, true), (true, false), (false, true)] {
        let (_temp, store) = store();
        let (body, _) = tool_history(&store, "gpt-6-astra").await;
        let first_socket = first_ws.then(|| store.contexts.open_socket().unwrap());
        let second_socket = second_ws.then(|| store.contexts.open_socket().unwrap());
        let transport = |ws| {
            if ws {
                EmulationTransport::WebSocket
            } else {
                EmulationTransport::Http
            }
        };
        let first = scoped(
            &store,
            body.clone(),
            &HeaderMap::new(),
            transport(first_ws),
            None,
            first_socket.as_ref().map(|s| s.id.as_str()),
            NAMESPACE,
            KEY,
        )
        .await
        .unwrap();
        store
            .contexts
            .learn_turn(first.operation.as_ref().unwrap(), "synthetic-first-route")
            .unwrap();
        let second = scoped(
            &store,
            body,
            &HeaderMap::new(),
            transport(second_ws),
            None,
            second_socket.as_ref().map(|s| s.id.as_str()),
            NAMESPACE,
            KEY,
        )
        .await
        .unwrap();
        let original = active(&store, &first);
        let fork = active(&store, &second);
        assert_eq!(original.identity.session_id, fork.identity.session_id);
        assert_eq!(original.identity.thread_id, fork.identity.thread_id);
        assert_eq!(original.identity.turn_id, fork.identity.turn_id);
        assert_ne!(original.branch, fork.branch);
        assert!(
            store
                .contexts
                .turn_token(second.operation.as_ref().unwrap())
                .is_none()
        );
        complete(
            &store,
            second,
            "resp_connection_second",
            text_output("second"),
        )
        .await;
        assert_eq!(store.contexts.inner.lock().unwrap().operations.len(), 1);
        complete(&store, first, "resp_connection_first", text_output("first")).await;
    }
}

#[tokio::test]
async fn invalid_previous_and_tool_references_still_fail_before_fork_admission() {
    let (_temp, store) = store();
    let (body, _) = tool_history(&store, "gpt-5.4").await;
    let first = prepare(&store, body.clone()).await.unwrap();
    for case in ["missing-previous", "missing-call", "duplicate-result"] {
        let mut invalid = body.clone();
        match case {
            "missing-previous" => invalid["previous_response_id"] = "resp_missing".into(),
            "missing-call" => invalid["input"][2]["call_id"] = "call_missing".into(),
            _ => {
                let result = invalid["input"][2].clone();
                invalid["input"].as_array_mut().unwrap().push(result);
            }
        }
        assert!(prepare(&store, invalid).await.is_err(), "{case}");
        let inner = store.contexts.inner.lock().unwrap();
        assert_eq!(inner.operations.len(), 1);
        assert!(inner.reservations.is_empty());
    }
    drop(first);
}

#[tokio::test]
async fn parallel_memory_extractions_keep_shared_identity_and_absent_turns() {
    let (_temp, store) = store();
    let requests = (0..8).map(|index| {
        let body = json!({"model":"gpt-5.4","input":format!("synthetic rollout {index}"),
            "client_metadata":{"session_id":"memory-session","thread_id":"memory-session",
                "x-codex-turn-metadata":json!({"request_kind":"memory"}).to_string()}});
        prepare(&store, body)
    });
    let prepared: Vec<_> = futures_util::future::join_all(requests)
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();
    let identity = prepared[0].resolved_identity.as_ref().unwrap();
    for (index, request) in prepared.iter().enumerate() {
        let record = active(&store, request);
        assert_eq!(record.identity.session_id, identity.session_id);
        assert_eq!(record.identity.thread_id, identity.thread_id);
        assert!(record.identity.turn_id.is_none());
        assert_eq!(record.identity.request_kind, "memory");
        assert_eq!(
            record.history.unwrap().values()[0]["content"][0]["text"],
            format!("synthetic rollout {index}")
        );
    }
    let inner = store.contexts.inner.lock().unwrap();
    let branches: std::collections::HashSet<_> = inner
        .operations
        .values()
        .map(|op| &op.record.branch)
        .collect();
    assert_eq!(branches.len(), 8);
    drop(inner);
    drop(prepared);
    assert!(store.contexts.inner.lock().unwrap().operations.is_empty());
}

fn compaction_body(mut body: Value) -> Value {
    body["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"compaction_trigger"}));
    body["client_metadata"] = json!({"x-codex-turn-metadata":json!({
        "request_kind":"compaction", "compaction":{"implementation":"responses_compaction_v2"}
    }).to_string()});
    body
}

#[tokio::test]
async fn overlapping_compactions_keep_separate_checkpoints_and_do_not_rewind_windows() {
    let (_temp, store) = store();
    let (body, _) = tool_history(&store, "gpt-5.4").await;
    let body = compaction_body(body);
    let first = prepare(&store, body.clone()).await.unwrap();
    let second = prepare(&store, body).await.unwrap();
    let target = first.pending_compaction.as_ref().unwrap().target_window;
    let thread = first.resolved_identity.as_ref().unwrap().thread_id.clone();
    assert_eq!(
        second.pending_compaction.as_ref().unwrap().target_window,
        target
    );
    assert_ne!(
        active(&store, &first).branch,
        active(&store, &second).branch
    );
    let checkpoint = |id, text| json!([{"type":"compaction","id":id,"encrypted_content":text}]);
    let second = complete(
        &store,
        second,
        "resp_compact_second",
        checkpoint("cmp_second", "synthetic-second"),
    )
    .await;
    let mut later = compaction_body(request(json!([input("later window")])));
    later["previous_response_id"] = second["id"].clone();
    let later = prepare(&store, later).await.unwrap();
    assert_eq!(
        later.pending_compaction.as_ref().unwrap().target_window,
        target + 1
    );
    complete(
        &store,
        later,
        "resp_compact_later",
        checkpoint("cmp_later", "synthetic-later"),
    )
    .await;
    let first = complete(
        &store,
        first,
        "resp_compact_first",
        checkpoint("cmp_first", "synthetic-first"),
    )
    .await;
    let current = store
        .edit(NAMESPACE, OWNER, KEY, move |editor| {
            Ok(editor.window_number(&thread))
        })
        .await
        .unwrap();
    assert_eq!(
        current,
        Some(target + 1),
        "late completion rewound shared window state"
    );
    for (response, own, foreign) in [
        (first, "synthetic-first", "synthetic-second"),
        (second, "synthetic-second", "synthetic-first"),
    ] {
        let mut next = request(json!([input("follow-up")]));
        next["previous_response_id"] = response["id"].clone();
        let next = prepare(&store, next).await.unwrap();
        assert_eq!(
            next.resolved_identity.as_ref().unwrap().window_number,
            target
        );
        let history = active(&store, &next).history.unwrap().values();
        assert!(history.iter().any(|item| item["encrypted_content"] == own));
        assert!(
            !history
                .iter()
                .any(|item| item["encrypted_content"] == foreign)
        );
    }
}

#[tokio::test]
async fn concurrent_prewarm_connections_hand_off_only_their_own_routing_token() {
    let (_temp, store) = store();
    let (mut body, _) = tool_history(&store, "gpt-6-astra").await;
    body["generate"] = false.into();
    let a = store.contexts.open_socket().unwrap();
    let b = store.contexts.open_socket().unwrap();
    let first = scoped(
        &store,
        body.clone(),
        &HeaderMap::new(),
        EmulationTransport::WebSocket,
        None,
        Some(&a.id),
        NAMESPACE,
        KEY,
    )
    .await
    .unwrap();
    let second = scoped(
        &store,
        body.clone(),
        &HeaderMap::new(),
        EmulationTransport::WebSocket,
        None,
        Some(&b.id),
        NAMESPACE,
        KEY,
    )
    .await
    .unwrap();
    let first_identity = first.resolved_identity.clone().unwrap();
    let second_identity = second.resolved_identity.clone().unwrap();
    assert_ne!(
        active(&store, &first).branch,
        active(&store, &second).branch
    );
    store
        .contexts
        .learn_response_turn(first.operation.as_ref().unwrap(), "synthetic-startup-a")
        .unwrap();
    store
        .contexts
        .learn_response_turn(second.operation.as_ref().unwrap(), "synthetic-startup-b")
        .unwrap();
    complete(&store, second, "resp_prewarm_b", json!([])).await;
    complete(&store, first, "resp_prewarm_a", json!([])).await;
    body.as_object_mut().unwrap().remove("generate");
    body["input"]
        .as_array_mut()
        .unwrap()
        .push(input("synthetic business"));
    for (socket, identity, token) in [
        (&a, &first_identity, "synthetic-startup-a"),
        (&b, &second_identity, "synthetic-startup-b"),
    ] {
        let request = scoped(
            &store,
            body.clone(),
            &HeaderMap::new(),
            EmulationTransport::WebSocket,
            Some(identity),
            Some(&socket.id),
            NAMESPACE,
            KEY,
        )
        .await
        .unwrap();
        assert_eq!(
            store
                .contexts
                .turn_token(request.operation.as_ref().unwrap())
                .as_deref(),
            Some(token)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&request.body).unwrap()["client_metadata"]["x-codex-turn-state"],
            token
        );
    }
}
