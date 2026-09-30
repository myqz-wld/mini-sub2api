use super::*;

#[tokio::test]
async fn startup_metadata_timing_and_ids_preserve_the_first_business_owner() {
    for timing in ["active", "idle", "admitted", "written"] {
        for id_shape in ["none", "response", "response_id"] {
            for token in ["synthetic-first", ""] {
                let (_temp, store) = store();
                let socket = store.contexts.open_socket().unwrap();
                let mut warm = request(json!([]));
                warm["generate"] = false.into();
                warm["client_metadata"] = json!({"session_id":"timing-session"});
                let warm = websocket_request(&store, warm, &socket.id, None)
                    .await
                    .unwrap();
                let warm_context = response_context(&store, &warm);
                warm_context
                    .translate_value(
                        json!({"type":"response.created","response":{"id":"timing-warm"}}),
                    )
                    .await
                    .unwrap();
                let metadata = |token| {
                    let mut event =
                        json!({"type":"response.metadata","headers":{"x-codex-turn-state":token}});
                    match id_shape {
                        "response" => event["response"] = json!({"id":"timing-warm"}),
                        "response_id" => event["response_id"] = "timing-warm".into(),
                        _ => {}
                    }
                    event
                };
                if timing == "active" {
                    warm_context.translate_value(metadata(token)).await.unwrap();
                }
                warm_context.translate_value(json!({"type":"response.completed","response":{"id":"timing-warm","output":[]}})).await.unwrap();
                if timing == "idle" {
                    warm_context.translate_value(metadata(token)).await.unwrap();
                }
                let mut body = request(json!([input("synthetic business")]));
                body["client_metadata"] =
                    json!({"session_id":"timing-session","turn_id":"timing-turn"});
                let first = websocket_request(
                    &store,
                    body.clone(),
                    &socket.id,
                    warm.resolved_identity.as_ref(),
                )
                .await
                .unwrap();
                let business_context = response_context(&store, &first);
                if timing == "admitted" {
                    // The reader may still hold the prewarm operation during admission.
                    warm_context.translate_value(metadata(token)).await.unwrap();
                } else if timing == "written" {
                    business_context
                        .translate_value(metadata(token))
                        .await
                        .unwrap();
                }
                business_context
                    .translate_value(metadata("later-must-not-replace"))
                    .await
                    .unwrap();
                assert_eq!(
                    store
                        .contexts
                        .turn_token(first.operation.as_ref().unwrap())
                        .as_deref(),
                    Some(token),
                    "{timing}/{id_shape}"
                );
                let created = business_context
                    .translate_value(
                        json!({"type":"response.created","response":{"id":"timing-business"}}),
                    )
                    .await
                    .unwrap();
                let completed = business_context.translate_value(json!({"type":"response.completed","response":{"id":"timing-business","output":[]}})).await.unwrap();
                assert_eq!(created["response"]["id"], completed["response"]["id"]);
                let next = websocket_request(
                    &store,
                    body.clone(),
                    &socket.id,
                    first.resolved_identity.as_ref(),
                )
                .await
                .unwrap();
                let next_wire: Value = serde_json::from_slice(&next.body).unwrap();
                assert_eq!(next_wire["client_metadata"]["x-codex-turn-state"], token);
                response_context(&store,&next).translate_value(json!({"type":"response.completed","response":{"id":"timing-continuation","output":[]}})).await.unwrap();
                body["client_metadata"]["turn_id"] = "second-turn".into();
                let second =
                    websocket_request(&store, body, &socket.id, next.resolved_identity.as_ref())
                        .await
                        .unwrap();
                // Consumed startup cannot be resurrected by its old context or explicit ID.
                warm_context
                    .translate_value(metadata("stale"))
                    .await
                    .unwrap();
                if id_shape != "none" {
                    response_context(&store, &second)
                        .translate_value(metadata("stale"))
                        .await
                        .unwrap();
                }
                assert!(
                    store
                        .contexts
                        .turn_token(second.operation.as_ref().unwrap())
                        .is_none()
                );
            }
        }
    }
}

#[tokio::test]
async fn completed_startup_handoff_is_invalidated_by_other_work_or_socket_close() {
    for mode in ["memory", "other-thread", "closed-socket", "failed"] {
        let (_temp, store) = store();
        let socket = store.contexts.open_socket().unwrap();
        let mut warm = request(json!([]));
        warm["generate"] = false.into();
        warm["client_metadata"] = json!({"session_id":"isolation-session"});
        let warm = websocket_request(&store, warm, &socket.id, None)
            .await
            .unwrap();
        let context = response_context(&store, &warm);
        context
            .translate_value(json!({"type":"response.created","response":{"id":"isolated-warm"}}))
            .await
            .unwrap();
        context.translate_value(json!({"type":if mode=="failed" {"response.failed"} else {"response.completed"},"response":{"id":"isolated-warm","output":[]}})).await.unwrap();
        if mode == "closed-socket" {
            store.contexts.socket_closed(&socket.id);
        }
        if matches!(mode, "memory" | "other-thread") {
            let mut other = request(json!([input("intervening work")]));
            other["client_metadata"] =
                json!({"session_id":"isolation-session","turn_id":"other-turn"});
            if mode == "other-thread" {
                other["client_metadata"]["thread_id"] = "other-thread".into();
                other["client_metadata"]["parent_thread_id"] = "isolation-session".into();
            } else {
                other["client_metadata"]["request_kind"] = "memory".into();
            }
            let other =
                websocket_request(&store, other, &socket.id, warm.resolved_identity.as_ref())
                    .await
                    .unwrap();
            response_context(&store,&other).translate_value(json!({"type":"response.completed","response":{"id":"intervening-response","output":[]}})).await.unwrap();
        }
        context.translate_value(json!({"type":"response.metadata","headers":{"x-codex-turn-state":"must-not-cross"}})).await.unwrap();
        let next_socket = store.contexts.open_socket().unwrap();
        let selected = if mode == "closed-socket" {
            &next_socket.id
        } else {
            &socket.id
        };
        let mut body = request(json!([input("next work")]));
        body["client_metadata"] = json!({"session_id":"isolation-session","turn_id":"next-turn"});
        let next = websocket_request(&store, body, selected, warm.resolved_identity.as_ref())
            .await
            .unwrap();
        assert!(
            store
                .contexts
                .turn_token(next.operation.as_ref().unwrap())
                .is_none(),
            "{mode}"
        );
    }
}

#[tokio::test]
async fn idle_prewarm_first_token_survives_completion() {
    let mut lost = 0;
    for token in ["synthetic-idle-token", ""] {
        let (_temp, store) = store();
        let socket = store.contexts.open_socket().unwrap();
        let mut warm = request(json!([]));
        warm["generate"] = false.into();
        warm["client_metadata"] = json!({"session_id":"probe-startup"});
        let prewarm = websocket_request(&store, warm, &socket.id, None)
            .await
            .unwrap();
        let identity = prewarm.resolved_identity.clone().unwrap();
        let response = response_context(&store, &prewarm);
        response
            .translate_value(json!({"type":"response.created","response":{"id":"probe-prewarm"}}))
            .await
            .unwrap();
        response
            .translate_value(
                json!({"type":"response.completed","response":{"id":"probe-prewarm","output":[]}}),
            )
            .await
            .unwrap();
        response
            .translate_value(
                json!({"type":"response.metadata","headers":{"x-codex-turn-state":token}}),
            )
            .await
            .unwrap();
        let mut body = request(json!([input("synthetic business")]));
        body["client_metadata"] =
            json!({"session_id":"probe-startup","turn_id":"probe-turn","x-codex-turn-state":token});
        let first = websocket_request(&store, body.clone(), &socket.id, Some(&identity))
            .await
            .unwrap();
        let first_wire: Value = serde_json::from_slice(&first.body).unwrap();
        let token_on_first = first_wire["client_metadata"]
            .get("x-codex-turn-state")
            .and_then(Value::as_str);
        assert_eq!(token_on_first, Some(token));
        response_context(&store, &first)
            .translate_value(
                json!({"type":"response.completed","response":{"id":"probe-business","output":[]}}),
            )
            .await
            .unwrap();
        let next = websocket_request(&store, body, &socket.id, first.resolved_identity.as_ref())
            .await
            .unwrap();
        let next_wire: Value = serde_json::from_slice(&next.body).unwrap();
        let token_on_next = next_wire["client_metadata"]
            .get("x-codex-turn-state")
            .and_then(Value::as_str);
        if token_on_next != Some(token) {
            lost += 1;
        }
    }
    assert_eq!(
        lost, 0,
        "idle prewarm metadata was consumed but not retained for business or continuation"
    );
}
