use super::*;

fn branch_request(
    session: &str,
    thread: &str,
    turn: &str,
    parent: Option<&str>,
    source: Option<&str>,
    previous: Option<&str>,
    input: Value,
) -> Value {
    let mut body = request(input);
    let mut metadata = json!({"session_id":session,"thread_id":thread,"turn_id":turn});
    if let Some(parent) = parent {
        metadata["parent_thread_id"] = parent.into();
    }
    if let Some(source) = source {
        metadata["x-codex-turn-metadata"] =
            json!({"forked_from_thread_id":source}).to_string().into();
    }
    body["client_metadata"] = metadata;
    if let Some(previous) = previous {
        body["previous_response_id"] = previous.into();
    }
    body
}

async fn finish(store: &RequestStateStore, prepared: &PreparedEmulatedRequest, id: &str) -> String {
    response_context(store, prepared)
        .translate_value(json!({"type":"response.completed","response":{"id":id,"output":[]}}))
        .await
        .unwrap()["response"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn remote_only_history_keeps_thread_eligibility_without_rebuilding_bodies() {
    let (_temp, store) = store();
    let socket = store.contexts.open_socket().unwrap();
    let first = websocket_request(
        &store,
        branch_request(
            "session",
            "child",
            "turn-first",
            Some("session"),
            None,
            None,
            json!([input("seed")]),
        ),
        &socket.id,
        None,
    )
    .await
    .unwrap();
    let identity = first.resolved_identity.as_ref().unwrap().clone();
    let previous = finish(&store, &first, "response-first").await;
    store
        .contexts
        .inner
        .lock()
        .unwrap()
        .expire(Instant::now() + Duration::from_secs(4 * 60 * 60));
    let same = websocket_request(
        &store,
        branch_request(
            "session",
            "child",
            "turn-next",
            Some("session"),
            None,
            Some(&previous),
            json!([input("suffix")]),
        ),
        &socket.id,
        Some(&identity),
    )
    .await
    .unwrap();
    let emitted: Value = serde_json::from_slice(&same.body).unwrap();
    assert_eq!(emitted["input"].as_array().unwrap().len(), 1);
    assert!(emitted["previous_response_id"].is_string());
    let next_identity = same.resolved_identity.as_ref().unwrap().clone();
    let next = finish(&store, &same, "response-next").await;
    let scope = ContextStore::scope_key(NAMESPACE, KEY);
    assert!(
        store.contexts.inner.lock().unwrap().scopes[&scope].records[&next]
            .history
            .is_none()
    );
    let unrelated = websocket_request(
        &store,
        branch_request(
            "session",
            "sibling",
            "turn-sibling",
            Some("session"),
            None,
            Some(&previous),
            json!([input("suffix")]),
        ),
        &socket.id,
        Some(&next_identity),
    )
    .await;
    assert!(matches!(
        unrelated,
        Err(StatefulPrepareError::InvalidRequest)
    ));
}

#[tokio::test]
async fn previous_lineage_validates_copied_fork_history_beyond_the_response_owner() {
    let (_temp, store) = store();
    let source = prepare(
        &store,
        branch_request(
            "source",
            "source",
            "source-turn",
            None,
            None,
            None,
            json!([input("source history")]),
        ),
    )
    .await
    .unwrap();
    finish(&store, &source, "response-source").await;
    let socket = store.contexts.open_socket().unwrap();
    let copied = json!([{"role":"user","content":"source history","internal_chat_message_metadata_passthrough":{"turn_id":"source-turn"}},input("fork work")]);
    let fork = websocket_request(
        &store,
        branch_request(
            "fork",
            "fork",
            "fork-turn",
            None,
            Some("source"),
            None,
            copied,
        ),
        &socket.id,
        None,
    )
    .await
    .unwrap();
    let identity = fork.resolved_identity.as_ref().unwrap().clone();
    let previous = finish(&store, &fork, "response-fork").await;
    store
        .contexts
        .inner
        .lock()
        .unwrap()
        .expire(Instant::now() + Duration::from_secs(4 * 60 * 60));
    let denied = websocket_request(
        &store,
        branch_request(
            "fork",
            "child",
            "child-turn",
            Some("fork"),
            None,
            Some(&previous),
            json!([input("suffix")]),
        ),
        &socket.id,
        Some(&identity),
    )
    .await;
    assert!(matches!(denied, Err(StatefulPrepareError::InvalidRequest)));
    let allowed = websocket_request(
        &store,
        branch_request(
            "fork",
            "child",
            "child-turn",
            Some("fork"),
            Some("source"),
            Some(&previous),
            json!([input("suffix")]),
        ),
        &socket.id,
        Some(&identity),
    )
    .await;
    assert!(allowed.is_ok());
}

#[tokio::test]
async fn previous_only_continuation_restores_its_own_fork_provenance() {
    for ws in [false, true] {
        let (_temp, store) = store();
        let source = prepare(
            &store,
            branch_request(
                "source",
                "source",
                "source-turn",
                None,
                None,
                None,
                json!([input("source history")]),
            ),
        )
        .await
        .unwrap();
        finish(&store, &source, "response-source").await;
        let socket = store.contexts.open_socket().unwrap();
        let copied = json!([{"role":"user","content":"source history","internal_chat_message_metadata_passthrough":{"turn_id":"source-turn"}},input("fork work")]);
        let fork = websocket_request(
            &store,
            branch_request(
                "fork",
                "fork",
                "fork-turn",
                None,
                Some("source"),
                None,
                copied,
            ),
            &socket.id,
            None,
        )
        .await
        .unwrap();
        let identity = fork.resolved_identity.as_ref().unwrap().clone();
        let previous = finish(&store, &fork, "response-fork").await;
        let mut delta = request(json!([input("suffix")]));
        delta["previous_response_id"] = previous.into();
        let next = if ws {
            websocket_request(&store, delta, &socket.id, Some(&identity)).await
        } else {
            prepare(&store, delta).await
        }
        .unwrap();
        assert_eq!(
            next.resolved_identity.unwrap().forked_from_thread_id,
            identity.forked_from_thread_id
        );
    }
}
