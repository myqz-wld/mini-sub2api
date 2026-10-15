//! Regression coverage for turn continuity, routing and anonymous setup changes.
use super::*;

#[path = "subscription_alignment_boundary_tests.rs"]
mod boundaries;

#[tokio::test]
async fn restart_and_expiry_preserve_implicit_tool_continuations() {
    for restart in [false, true] {
        for new_user in [false, true] {
            let (temp, initial) = store();
            let mut body = request(json!([input("synthetic tool task")]));
            body["client_metadata"] = json!({"session_id":"synthetic-session"});
            let seed = prepare(&initial, body.clone()).await.unwrap();
            let original = seed.resolved_identity.as_ref().unwrap().clone();
            let response = publish(
                &initial,
                seed,
                "resp_assessment_tool",
                json!([
                    {"type":"function_call","id":"fc_assessment","call_id":"call_assessment",
                     "name":"tool","arguments":"{}","status":"completed"}
                ]),
            )
            .await;
            body["input"] = json!([input("synthetic tool task"), response["output"][0],
                {"type":"function_call_output","call_id":response["output"][0]["call_id"],"output":"synthetic result"}]);
            if new_user {
                body["input"]
                    .as_array_mut()
                    .unwrap()
                    .push(input("synthetic next task"));
            }
            let next_store = if restart {
                drop(initial);
                RequestStateStore::new(temp.path().to_path_buf())
            } else {
                initial.contexts.inner.lock().unwrap().expire(
                    std::time::Instant::now() + std::time::Duration::from_secs(4 * 60 * 60),
                );
                initial
            };
            let expected_thread = original.thread_id.clone();
            let expected_turn = original.turn_id.clone();
            next_store
                .edit(NAMESPACE, OWNER, KEY, move |editor| {
                    assert_eq!(editor.current_turn_id(&expected_thread), expected_turn);
                    Ok(())
                })
                .await
                .unwrap();
            let continued = prepare(&next_store, body).await.unwrap();
            let actual = continued.resolved_identity.as_ref().unwrap();
            assert_eq!(actual.session_id, original.session_id);
            assert_eq!(actual.thread_id, original.thread_id);
            assert_eq!(
                actual.turn_id == original.turn_id,
                !new_user,
                "restart={restart} new_user={new_user}"
            );
        }
    }
}

#[tokio::test]
async fn child_continuations_never_execute_the_parent_turn() {
    for explicit_turn in [false, true] {
        for new_user in [false, true] {
            let (_temp, store) = store();
            let mut parent = request(json!([input("synthetic parent task")]));
            parent["client_metadata"] = json!({"session_id":"synthetic-family"});
            let seed = prepare(&store, parent).await.unwrap();
            let original = seed.resolved_identity.as_ref().unwrap().clone();
            let response = publish(&store, seed, "resp_assessment_parent", json!([
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic answer"}]}
            ])).await;
            let mut child = request(if new_user {
                json!([input("synthetic child task")])
            } else {
                json!([])
            });
            child["previous_response_id"] = response["id"].clone();
            child["client_metadata"] = json!({"session_id":"synthetic-family", "thread_id":"synthetic-child", "parent_thread_id":"synthetic-family"});
            if explicit_turn {
                child["client_metadata"]["turn_id"] = "synthetic-child-turn".into();
            }
            let result = prepare(&store, child).await;
            let continued = result.expect("child continuation has its own turn");
            let actual = continued.resolved_identity.as_ref().unwrap();
            assert_eq!(actual.session_id, original.session_id);
            assert_ne!(actual.thread_id, original.thread_id);
            assert_ne!(actual.turn_id, original.turn_id);
        }
    }
}

#[tokio::test]
async fn restart_restores_trusted_token_with_explicit_turn_and_full_input() {
    let (temp, initial) = store();
    let mut body = request(json!([input("synthetic task")]));
    body["client_metadata"] = json!({"session_id":"synthetic-session", "turn_id":"synthetic-turn"});
    let seed = prepare(&initial, body.clone()).await.unwrap();
    let original = seed.resolved_identity.as_ref().unwrap().clone();
    initial
        .learn_turn_state(
            NAMESPACE,
            seed.operation.as_ref().unwrap(),
            "synthetic-routing-token",
        )
        .await
        .unwrap();
    drop(seed);
    let retry = prepare(&initial, body.clone()).await.unwrap();
    assert_eq!(
        retry.headers["x-codex-turn-state"],
        "synthetic-routing-token"
    );
    drop(retry);
    drop(initial);
    let restarted = RequestStateStore::new(temp.path().to_path_buf());
    body["client_metadata"]["x-codex-turn-state"] = "untrusted-caller-token".into();
    let retry = prepare(&restarted, body).await.unwrap();
    assert_eq!(
        retry.resolved_identity.as_ref().unwrap().turn_id,
        original.turn_id
    );
    assert_eq!(
        retry.headers["x-codex-turn-state"],
        "synthetic-routing-token"
    );
}

#[tokio::test]
async fn busy_branch_inherits_known_empty_or_absent_token_on_both_transports() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for token in [None, Some(""), Some("synthetic-known-token")] {
            let (_temp, store) = store();
            let first_socket = store.contexts.open_socket().unwrap();
            let second_socket = store.contexts.open_socket().unwrap();
            let mut body = request(json!([input("synthetic task")]));
            body["client_metadata"] =
                json!({"session_id":"synthetic-session", "turn_id":"synthetic-turn"});
            let first = prepare(&store, body.clone()).await.unwrap();
            if let Some(token) = token {
                store
                    .contexts
                    .learn_turn(first.operation.as_ref().unwrap(), token)
                    .unwrap();
            }
            let second = prepare_stateful_codex_request(
                UpstreamProfile::CodexSubscription1620,
                transport,
                &HeaderMap::new(),
                Bytes::from(serde_json::to_vec(&body).unwrap()),
                128 * 1024 * 1024,
                CodexStateContext {
                    force_lite: false,
                    admission: None,
                    store: &store,
                    state_namespace: NAMESPACE,
                    account_ref: OWNER,
                    downstream_scope: KEY,
                    fingerprint_mode: FingerprintMode::Device,
                    binding: None,
                    socket_id: (transport == EmulationTransport::WebSocket)
                        .then_some(second_socket.id.as_str()),
                },
                false,
            )
            .await
            .unwrap();
            assert_eq!(
                first.resolved_identity.as_ref().unwrap().turn_id,
                second.resolved_identity.as_ref().unwrap().turn_id
            );
            let inner = store.contexts.inner.lock().unwrap();
            assert_ne!(
                inner.operations[&first.operation.as_ref().unwrap().0.id]
                    .record
                    .branch,
                inner.operations[&second.operation.as_ref().unwrap().0.id]
                    .record
                    .branch
            );
            drop(inner);
            assert_eq!(
                store
                    .contexts
                    .turn_token(first.operation.as_ref().unwrap())
                    .as_deref(),
                token
            );
            assert_eq!(
                store
                    .contexts
                    .turn_token(second.operation.as_ref().unwrap())
                    .as_deref(),
                token
            );
            assert_eq!(
                second
                    .headers
                    .get("x-codex-turn-state")
                    .and_then(|v| v.to_str().ok()),
                token
            );
            let wire: Value = serde_json::from_slice(&second.body).unwrap();
            assert_eq!(
                wire["client_metadata"]
                    .get("x-codex-turn-state")
                    .and_then(Value::as_str),
                if transport == EmulationTransport::WebSocket {
                    token
                } else {
                    None
                }
            );
            drop(first_socket);
        }
    }
}

#[tokio::test]
async fn known_output_alias_preserves_session_across_leading_setup_changes() {
    for historical_prefix in [false, true] {
        let (_temp, store) = store();
        let mut body = request(json!([
            {"type":"message", "role":"developer", "content":"synthetic historical base"},
            input("synthetic task")
        ]));
        let seed = prepare(&store, body.clone()).await.unwrap();
        let original = seed.resolved_identity.as_ref().unwrap().clone();
        let response = publish(&store, seed, "resp_assessment_prefix", json!([
            {"type":"message","id":"msg_synthetic_prefix","role":"assistant","content":[{"type":"output_text","text":"synthetic answer"}]}
        ])).await;
        body["input"]
            .as_array_mut()
            .unwrap()
            .extend([response["output"][0].clone(), input("synthetic next task")]);
        if historical_prefix {
            body["input"][0]["content"] = "synthetic changed historical base".into();
        } else {
            body["instructions"] = "synthetic changed current base".into();
        }
        let next = prepare(&store, body).await.unwrap();
        assert_eq!(
            next.resolved_identity.as_ref().unwrap().session_id,
            original.session_id
        );
    }
}
