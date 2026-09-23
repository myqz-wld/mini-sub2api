use super::*;

const META: &str = "internal_chat_message_metadata_passthrough";

#[tokio::test]
async fn long_routing_tokens_are_charged_and_keep_the_first_value() {
    let (_temp, store) = store();
    let prepared = prepare(&store, request(json!([input("routing budget")])))
        .await
        .unwrap();
    let operation = prepared.operation.as_ref().unwrap();
    let session = &prepared.resolved_identity.as_ref().unwrap().session_id;
    let key = ContextStore::scope_key(NAMESPACE, KEY);
    let before = {
        let inner = store.contexts.inner.lock().unwrap();
        (
            inner.scopes[&key].cost(),
            inner.scopes[&key].session_cost(session),
        )
    };
    let token = "synthetic-opaque-token-".repeat(200);
    store.contexts.learn_turn(operation, &token).unwrap();
    store
        .contexts
        .learn_turn(
            operation,
            &"x".repeat(crate::subscription_routing::MAX_ROUTING_TOKEN_BYTES + 1),
        )
        .unwrap();
    assert_eq!(
        store.contexts.turn_token(operation).as_deref(),
        Some(token.as_str())
    );
    let inner = store.contexts.inner.lock().unwrap();
    assert!(inner.scopes[&key].cost() >= before.0 + token.len());
    assert!(inner.scopes[&key].session_cost(session) >= before.1 + token.len());
}

#[tokio::test]
async fn routing_tokens_cannot_exceed_an_active_sessions_memory_budget() {
    let (_temp, mut store) = store();
    let mut limits = (*store.contexts.limits).clone();
    limits.global_bytes = 32 * 1024;
    limits.key_bytes = 32 * 1024;
    limits.session_bytes = 32 * 1024;
    store.contexts.limits = std::sync::Arc::new(limits);
    let prepared = prepare(&store, request(json!([input("routing budget")])))
        .await
        .unwrap();
    let operation = prepared.operation.as_ref().unwrap();
    assert!(
        store
            .contexts
            .learn_turn(operation, &"x".repeat(48 * 1024))
            .is_err()
    );
    assert!(store.contexts.turn_token(operation).is_none());
}

#[tokio::test]
async fn caller_time_survives_without_an_item_id_in_responses_and_lite() {
    for model in ["gpt-5.4", "gpt-5.6-sol"] {
        let (_temp, store) = store();
        let mut item = input("timestamp");
        item[META] = json!({"create_time":1234.125});
        let mut body = request(json!([item]));
        body["model"] = json!(model);
        let prepared = prepare(&store, body).await.unwrap();
        let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
        let item = wire["input"].as_array().unwrap().last().unwrap();
        assert_eq!(item[META]["create_time"], json!(1234.125));
    }
}

#[tokio::test]
async fn reference_expansion_preserves_the_first_generated_time() {
    for with_id in [false, true] {
        let (_temp, store) = store();
        let mut item = input("first");
        if with_id {
            item["id"] = json!("msg_first");
        }
        let first = prepare(&store, request(json!([item]))).await.unwrap();
        let wire: Value = serde_json::from_slice(&first.body).unwrap();
        let response = publish(&store, first, "resp_time", json!([])).await;
        let mut next = request(json!([input("next")]));
        next["previous_response_id"] = response["id"].clone();
        // The second turn generates a new item lookup key even when both requests share a clock tick.
        let first_time = wire["input"][0][META]["create_time"].clone();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let next = prepare(&store, next).await.unwrap();
        let next: Value = serde_json::from_slice(&next.body).unwrap();
        assert_eq!(next["input"][0][META]["create_time"], first_time);
        assert_eq!(next["input"][0]["id"], wire["input"][0]["id"]);
    }
}

#[tokio::test]
async fn failed_turn_keeps_its_first_routing_token_until_expiry() {
    for created in [false, true] {
        let (_temp, store) = store();
        let mut body = request(json!([input("retry")]));
        body["client_metadata"] = json!({"session_id":"session","turn_id":"turn"});
        let first = prepare(&store, body.clone()).await.unwrap();
        store
            .contexts
            .learn_turn(first.operation.as_ref().unwrap(), "first-token")
            .unwrap();
        let state = ResponseStateContext::new(
            OWNER,
            NAMESPACE,
            KEY,
            &store,
            first.resolved_identity.as_ref(),
            None,
        )
        .with_operation(first.operation);
        state
            .translate_value(
                json!({"type":"response.metadata","headers":{"x-codex-turn-state":"ignored-event-token"}}),
            )
            .await
            .unwrap();
        if created {
            state
                .translate_value(json!({"type":"response.created","response":{"id":"resp_failed"}}))
                .await
                .unwrap();
        }
        state
            .translate_value(json!({"type":"error","error":{"code":"synthetic"}}))
            .await
            .unwrap();
        drop(state);
        let retry = prepare(&store, body).await.unwrap();
        let op = retry.operation.as_ref().unwrap();
        store.contexts.learn_turn(op, "second-token").unwrap();
        assert_eq!(
            store.contexts.turn_token(op).as_deref(),
            Some("first-token")
        );
        drop(retry);
        let mut inner = store.contexts.inner.lock().unwrap();
        inner.expire(std::time::Instant::now() + crate::subscription_context::HISTORY_TTL);
        assert!(inner.scopes.values().all(|s| s.routing.is_empty()));
    }
}

#[tokio::test]
async fn failed_routing_metadata_remains_reclaimable_under_capacity_pressure() {
    let (_temp, store) = store();
    for session in ["one", "two"] {
        let mut body = request(json!([input("retry")]));
        body["client_metadata"] = json!({"session_id":session,"turn_id":session});
        let prepared = prepare(&store, body).await.unwrap();
        store
            .contexts
            .learn_turn(prepared.operation.as_ref().unwrap(), "synthetic-token")
            .unwrap();
        drop(prepared);
    }
    let key = ContextStore::scope_key(NAMESPACE, KEY);
    let mut inner = store.contexts.inner.lock().unwrap();
    assert_eq!(inner.scopes[&key].routing.len(), 2);
    let mut limits = (*store.contexts.limits).clone();
    limits.global_bytes = 8192;
    assert!(inner.make_room(&limits, &key, "new-session", 0));
    assert_eq!(inner.scopes[&key].routing.len(), 1);
}

#[tokio::test]
async fn conflicting_or_malformed_footer_never_completes_or_publishes_history() {
    let item = json!({"id":"msg_output","type":"message","role":"assistant","content":[{"type":"output_text","text":"first"}]});
    let mut changed = item.clone();
    changed["content"][0]["text"] = json!("changed");
    for output in [json!([changed]), json!({}), json!(null), json!([false])] {
        let (_temp, store) = store();
        let prepared = prepare(&store, request(json!([input("start")])))
            .await
            .unwrap();
        let state = ResponseStateContext::new(
            OWNER,
            NAMESPACE,
            KEY,
            &store,
            prepared.resolved_identity.as_ref(),
            None,
        )
        .with_operation(prepared.operation);
        let created = state
            .translate_value(json!({"type":"response.created","response":{"id":"resp_invalid"}}))
            .await
            .unwrap();
        state
            .translate_value(
                json!({"type":"response.output_item.done","output_index":0,"item":item}),
            )
            .await
            .unwrap();
        assert!(state.translate_value(json!({"type":"response.completed","response":{"id":"resp_invalid","output":output}})).await.is_err(), "invalid completion was public");
        let mut next = request(json!([input("next")]));
        next["previous_response_id"] = created["response"]["id"].clone();
        assert!(matches!(
            prepare(&store, next).await,
            Err(StatefulPrepareError::StateUnavailable)
        ));
    }
}
