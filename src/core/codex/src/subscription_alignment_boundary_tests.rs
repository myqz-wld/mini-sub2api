use super::*;

#[tokio::test]
async fn late_startup_metadata_persists_its_bound_business_turn() {
    let (temp, initial) = store();
    let mut startup = request(json!([]));
    startup["generate"] = false.into();
    startup["client_metadata"] = json!({"session_id":"late-startup"});
    let startup = conflict_tests::prepare_websocket(&initial, startup)
        .await
        .unwrap();
    let late = ResponseStateContext::new(
        OWNER,
        NAMESPACE,
        KEY,
        &initial,
        startup.resolved_identity.as_ref(),
        None,
    )
    .with_operation(startup.operation.clone());
    publish(&initial, startup, "resp_late_startup", json!([])).await;
    let mut body = request(json!([input("synthetic business")]));
    body["client_metadata"] = json!({"session_id":"late-startup","turn_id":"late-business-turn"});
    let business = conflict_tests::prepare_websocket(&initial, body.clone())
        .await
        .unwrap();
    assert!(business.headers.get("x-codex-turn-state").is_none());
    late.translate_value(json!({"type":"response.metadata", "headers":{"x-codex-turn-state":"synthetic-late-token"}})).await.unwrap();
    drop(late);
    drop(business);
    drop(initial);
    let restarted = RequestStateStore::new(temp.path().into());
    let resumed = conflict_tests::prepare_websocket(&restarted, body)
        .await
        .unwrap();
    assert_eq!(
        resumed.headers["x-codex-turn-state"],
        "synthetic-late-token"
    );
}

#[tokio::test]
async fn fork_token_copy_is_charged_and_failed_admission_keeps_only_the_source() {
    for with_token in [false, true] {
        let (_temp, mut store) = store();
        let mut body = request(json!([input("synthetic capacity task")]));
        body["client_metadata"] =
            json!({"session_id":"capacity-session", "turn_id":"capacity-turn"});
        let first = prepare(&store, body.clone()).await.unwrap();
        let token = "x".repeat(crate::subscription_routing::MAX_ROUTING_TOKEN_BYTES);
        if with_token {
            store
                .contexts
                .learn_turn(first.operation.as_ref().unwrap(), &token)
                .unwrap();
        }
        let scope = ContextStore::scope_key(NAMESPACE, KEY);
        let session = &first.resolved_identity.as_ref().unwrap().session_id;
        let used = {
            let inner = store.contexts.inner.lock().unwrap();
            inner.scopes[&scope].session_cost(session) + inner.reserved(Some(&scope), Some(session))
        };
        let mut limits = (*store.contexts.limits).clone();
        limits.session_bytes = used + 48 * 1024;
        store.contexts.limits = std::sync::Arc::new(limits);
        let second = prepare(&store, body).await;
        if with_token {
            assert!(matches!(
                second,
                Err(StatefulPrepareError::StateUnavailable)
            ));
            assert_eq!(
                store
                    .contexts
                    .turn_token(first.operation.as_ref().unwrap())
                    .as_deref(),
                Some(token.as_str())
            );
            let inner = store.contexts.inner.lock().unwrap();
            assert_eq!(inner.scopes[&scope].routing.len(), 1);
            assert!(inner.reservations.is_empty());
        } else {
            assert!(second.is_ok());
        }
    }
}

#[tokio::test]
async fn completed_conflict_fork_replays_the_inherited_token() {
    let (_temp, store) = store();
    let mut body = request(json!([input("synthetic continuation task")]));
    body["client_metadata"] =
        json!({"session_id":"continuation-session", "turn_id":"continuation-turn"});
    let first = prepare(&store, body.clone()).await.unwrap();
    store
        .contexts
        .learn_turn(first.operation.as_ref().unwrap(), "synthetic-inherited")
        .unwrap();
    let fork = prepare(&store, body).await.unwrap();
    let response = publish(&store, fork, "resp_fork_continuation", json!([])).await;
    let mut next = request(json!([]));
    next["previous_response_id"] = response["id"].clone();
    let continued = prepare(&store, next).await.unwrap();
    assert_eq!(
        continued.headers["x-codex-turn-state"],
        "synthetic-inherited"
    );
    assert_eq!(
        continued.resolved_identity.as_ref().unwrap().turn_id,
        first.resolved_identity.as_ref().unwrap().turn_id
    );
}

#[tokio::test]
async fn late_tool_result_selects_its_call_turn_before_a_newer_current_pointer() {
    let (temp, initial) = store();
    let mut body = request(json!([input("synthetic tool task")]));
    body["client_metadata"] = json!({"session_id":"synthetic-session"});
    let seed = prepare(&initial, body.clone()).await.unwrap();
    let original = seed.resolved_identity.as_ref().unwrap().clone();
    let response = publish(&initial, seed, "resp_late_call", json!([
        {"type":"function_call","id":"fc_late","call_id":"call_late", "name":"tool","arguments":"{}",
         "internal_chat_message_metadata_passthrough":{"turn_id":original.turn_id}}
    ])).await;
    let mut newer = request(json!([input("synthetic newer task")]));
    newer["client_metadata"] = body["client_metadata"].clone();
    let newer = prepare(&initial, newer).await.unwrap();
    assert_ne!(
        newer.resolved_identity.as_ref().unwrap().turn_id,
        original.turn_id
    );
    drop(newer);
    drop(initial);
    let restarted = RequestStateStore::new(temp.path().into());
    body["input"] = json!([input("synthetic tool task"), response["output"][0],
        {"type":"function_call_output","call_id":response["output"][0]["call_id"],"output":"synthetic result"}]);
    let continued = prepare(&restarted, body).await.unwrap();
    assert_eq!(
        continued.resolved_identity.as_ref().unwrap().turn_id,
        original.turn_id
    );
}

#[tokio::test]
async fn detached_tool_history_continues_its_imported_target_turn() {
    let (temp, initial) = store();
    let body = request(json!([input("synthetic tool task")]));
    let seed = prepare(&initial, body.clone()).await.unwrap();
    let source = seed.resolved_identity.as_ref().unwrap().clone();
    let response = publish(&initial, seed, "resp_import_call", json!([
        {"type":"function_call","id":"fc_import","call_id":"call_import", "name":"tool","arguments":"{}",
         "internal_chat_message_metadata_passthrough":{"turn_id":source.turn_id}}
    ])).await;
    drop(initial);
    let restarted = RequestStateStore::new(temp.path().into());
    let mut body = body;
    body["input"] = json!([input("synthetic tool task"), response["output"][0],
        {"type":"function_call_output","call_id":response["output"][0]["call_id"],"output":"synthetic result"}]);
    let replay = prepare(&restarted, body).await.unwrap();
    let target = replay.resolved_identity.as_ref().unwrap();
    let wire: Value = serde_json::from_slice(&replay.body).unwrap();
    let call = wire["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call")
        .unwrap();
    assert_ne!(target.session_id, source.session_id);
    assert_ne!(target.turn_id, source.turn_id);
    assert_eq!(
        call["internal_chat_message_metadata_passthrough"]["turn_id"],
        target.turn_id.clone().unwrap()
    );
}

#[tokio::test]
async fn child_retry_is_stable_and_two_children_do_not_share_turns() {
    let (_temp, store) = store();
    let mut parent = request(json!([input("synthetic parent")]));
    parent["client_metadata"] = json!({"session_id":"family"});
    let seed = prepare(&store, parent).await.unwrap();
    let response = publish(&store, seed, "resp_parent_stable", json!([])).await;
    let mut body = request(json!([]));
    body["previous_response_id"] = response["id"].clone();
    body["client_metadata"] =
        json!({"session_id":"family","thread_id":"child-a","parent_thread_id":"family"});
    let first = prepare(&store, body.clone()).await.unwrap();
    let retry = prepare(&store, body.clone()).await.unwrap();
    assert_eq!(
        first.resolved_identity.as_ref().unwrap().turn_id,
        retry.resolved_identity.as_ref().unwrap().turn_id
    );
    body["client_metadata"]["thread_id"] = "child-b".into();
    let second = prepare(&store, body).await.unwrap();
    assert_ne!(
        first.resolved_identity.as_ref().unwrap().turn_id,
        second.resolved_identity.as_ref().unwrap().turn_id
    );
}

#[tokio::test]
async fn websocket_metadata_routing_survives_restart_and_is_emitted_in_create() {
    for token in ["", "synthetic-ws-token"] {
        let (temp, initial) = store();
        let mut body = request(json!([input("synthetic ws task")]));
        body["client_metadata"] = json!({"session_id":"ws-session", "turn_id":"ws-turn"});
        let prepared = conflict_tests::prepare_websocket(&initial, body.clone())
            .await
            .unwrap();
        let response = ResponseStateContext::new(
            OWNER,
            NAMESPACE,
            KEY,
            &initial,
            prepared.resolved_identity.as_ref(),
            None,
        )
        .with_operation(prepared.operation.clone());
        response
            .translate_value(
                json!({"type":"response.metadata", "headers":{"x-codex-turn-state":[token]}}),
            )
            .await
            .unwrap();
        drop(response);
        drop(prepared);
        drop(initial);
        let restarted = RequestStateStore::new(temp.path().into());
        body["client_metadata"]["x-codex-turn-state"] = "untrusted".into();
        let resumed = conflict_tests::prepare_websocket(&restarted, body)
            .await
            .unwrap();
        assert_eq!(resumed.headers["x-codex-turn-state"], token);
        let wire: Value = serde_json::from_slice(&resumed.body).unwrap();
        assert_eq!(wire["client_metadata"]["x-codex-turn-state"], token);
        assert!(
            !restarted
                .contexts
                .allow_hidden_prewarm(resumed.operation.as_ref().unwrap())
        );
    }
}

#[tokio::test]
async fn prewarm_cadence_is_per_thread_and_expires_with_idle_activity() {
    let (_temp, store) = store();
    let mut body = request(json!([input("synthetic task")]));
    body["client_metadata"] = json!({"session_id":"warm-session"});
    let first = prepare(&store, body.clone()).await.unwrap();
    assert!(
        store
            .contexts
            .allow_hidden_prewarm(first.operation.as_ref().unwrap())
    );
    let thread = first.resolved_identity.as_ref().unwrap().thread_id.clone();
    store
        .contexts
        .inner
        .lock()
        .unwrap()
        .scopes
        .get_mut(&ContextStore::scope_key(NAMESPACE, KEY))
        .unwrap()
        .thread_activity
        .get_mut(&thread)
        .unwrap()
        .1 = std::time::Instant::now() - std::time::Duration::from_secs(4 * 60 * 60);
    publish(&store, first, "resp_long_business_activity", json!([])).await;
    let next = prepare(&store, body.clone()).await.unwrap();
    assert!(
        !store
            .contexts
            .allow_hidden_prewarm(next.operation.as_ref().unwrap())
    );
    drop(next);
    let mut child = body.clone();
    child["client_metadata"]["thread_id"] = "new-child".into();
    child["client_metadata"]["parent_thread_id"] = "warm-session".into();
    let child = prepare(&store, child).await.unwrap();
    assert!(
        store
            .contexts
            .allow_hidden_prewarm(child.operation.as_ref().unwrap())
    );
    drop(child);
    store
        .contexts
        .inner
        .lock()
        .unwrap()
        .expire(std::time::Instant::now() + std::time::Duration::from_secs(4 * 60 * 60));
    let resumed = prepare(&store, body).await.unwrap();
    assert!(
        store
            .contexts
            .allow_hidden_prewarm(resumed.operation.as_ref().unwrap())
    );
}
