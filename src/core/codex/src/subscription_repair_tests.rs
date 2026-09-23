use super::*;
use crate::request_identity_projection::ResolvedRequestIdentity;
use std::time::{Duration, Instant};

async fn websocket_request(
    store: &RequestStateStore,
    body: Value,
    socket: &str,
    binding: Option<&ResolvedRequestIdentity>,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription1560,
        EmulationTransport::WebSocket,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        128 * 1024 * 1024,
        CodexStateContext {
            force_lite: false,
            admission: None,
            store,
            state_namespace: NAMESPACE,
            account_ref: OWNER,
            downstream_scope: KEY,
            fingerprint_mode: FingerprintMode::Device,
            binding,
            socket_id: Some(socket),
        },
        false,
    )
    .await
}

fn response_context(
    store: &RequestStateStore,
    prepared: &PreparedEmulatedRequest,
) -> ResponseStateContext {
    ResponseStateContext::new(
        OWNER,
        NAMESPACE,
        KEY,
        store,
        prepared.resolved_identity.as_ref(),
        prepared.pending_compaction.as_ref(),
    )
    .with_operation(prepared.operation.clone())
}

#[tokio::test]
async fn synthesized_ws_routing_token_is_not_pinned_to_the_last_field() {
    let (_temp, store) = store();
    let token = "synthetic-routing-token".repeat(40);
    let mut observed_nonterminal_slot = false;
    // Independent synthesized maps exercise the old deterministic append-at-end regression.
    for sample in 0..16 {
        let socket = store.contexts.open_socket().unwrap();
        let mut body = request(json!([input("synthetic order probe")]));
        body["client_metadata"] = json!({
            "session_id":format!("order-{sample}"),
            "turn_id":format!("turn-{sample}")
        });
        let first = websocket_request(&store, body.clone(), &socket.id, None)
            .await
            .unwrap();
        store
            .contexts
            .learn_turn(first.operation.as_ref().unwrap(), &token)
            .unwrap();
        response_context(&store,&first).translate_value(json!({"type":"response.completed","response":{"id":format!("resp_order_{sample}"),"output":[]}})).await.unwrap();
        let next = websocket_request(&store, body, &socket.id, first.resolved_identity.as_ref())
            .await
            .unwrap();
        assert!(!next.native_client_metadata);
        let value: Value = serde_json::from_slice(&next.body).unwrap();
        let metadata = value["client_metadata"].as_object().unwrap();
        assert_eq!(metadata["x-codex-turn-state"], token);
        observed_nonterminal_slot |=
            metadata.keys().next_back().map(String::as_str) != Some("x-codex-turn-state");
    }
    assert!(
        observed_nonterminal_slot,
        "routing state must participate in native-style randomized metadata ordering"
    );
}

#[tokio::test]
async fn bound_header_only_child_keeps_its_branch_but_uses_the_later_frames_window() {
    let (_temp, store) = store();
    let socket = store.contexts.open_socket().unwrap();
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("session-id", "root"),
        ("thread-id", "child"),
        ("x-codex-parent-thread-id", "root"),
        ("x-openai-subagent", "collab_spawn"),
        ("x-codex-window-id", "child:9"),
    ] {
        headers.insert(name, value.parse().unwrap());
    }
    let mut binding = None;
    for number in [9, 10] {
        let mut body = request(json!([input("header-only branch")]));
        if number == 10 {
            body["client_metadata"] =
                json!({"turn_id":"later-turn","x-codex-window-id":"child:10"});
        }
        let prepared = prepare_stateful_codex_request(
            UpstreamProfile::CodexSubscription1560,
            EmulationTransport::WebSocket,
            &headers,
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
                binding: binding.as_ref(),
                socket_id: Some(&socket.id),
            },
            false,
        )
        .await
        .unwrap();
        let identity = prepared.resolved_identity.as_ref().unwrap();
        assert_ne!(identity.session_id, identity.thread_id);
        assert_eq!(identity.window_number, number);
        if let Some(previous) = &binding {
            assert_eq!(identity.thread_id, previous.thread_id);
            assert_eq!(identity.parent_thread_id, previous.parent_thread_id);
        }
        binding = Some(identity.clone());
        response_context(&store, &prepared)
            .translate_value(json!({"type":"response.completed",
            "response":{"id":format!("resp_header_{number}"),"output":[]}}))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn startup_routing_is_adopted_once_only_after_completion_on_its_own_socket_and_thread() {
    for mode in [
        "completed",
        "failed",
        "other-socket",
        "other-thread",
        "expired-history",
    ] {
        let (_temp, store) = store();
        let socket = store.contexts.open_socket().unwrap();
        let mut body = request(json!([]));
        body["generate"] = false.into();
        body["client_metadata"] = json!({"session_id":"startup"});
        let prewarm = websocket_request(&store, body, &socket.id, None)
            .await
            .unwrap();
        let identity = prewarm.resolved_identity.as_ref().unwrap().clone();
        let operation = prewarm.operation.as_ref().unwrap();
        store
            .contexts
            .learn_turn(operation, "ignored-handshake")
            .unwrap();
        let response = response_context(&store, &prewarm);
        response
            .translate_value(json!({"type":"response.created","response":{"id":"resp_startup"}}))
            .await
            .unwrap();
        let startup = "startup-first".repeat(80);
        for token in [startup.as_str(), "startup-later"] {
            response
                .translate_value(
                    json!({"type":"response.metadata","headers":{"x-codex-turn-state":token}}),
                )
                .await
                .unwrap();
        }
        assert!(
            store.contexts.turn_token(operation).is_none(),
            "prewarm must not create an active turn"
        );
        response
            .translate_value(
                json!({"type": if mode == "failed" {"response.failed"} else {"response.completed"},
            "response":{"id":"resp_startup","output":[]}}),
            )
            .await
            .unwrap();
        if mode == "expired-history" {
            let mut inner = store.contexts.inner.lock().unwrap();
            inner.expire(Instant::now() + Duration::from_secs(4 * 60 * 60));
        }
        let other = store.contexts.open_socket().unwrap();
        let selected_socket = if mode == "other-socket" {
            &other.id
        } else {
            &socket.id
        };
        let mut business = request(json!([input("first business")]));
        business["client_metadata"] = json!({"session_id":"startup","turn_id":"turn-one","x-codex-turn-state":"untrusted-caller"});
        if mode == "other-thread" {
            business["client_metadata"]["thread_id"] = "child".into();
            business["client_metadata"]["parent_thread_id"] = "startup".into();
        }
        let first = websocket_request(&store, business, selected_socket, Some(&identity))
            .await
            .unwrap();
        let expected = if matches!(mode, "completed" | "expired-history") {
            Some(startup.as_str())
        } else {
            None
        };
        assert_eq!(
            first
                .headers
                .get("x-codex-turn-state")
                .map(|v| v.to_str().unwrap()),
            expected,
            "{mode}"
        );
        let operation = first.operation.as_ref().unwrap();
        store
            .contexts
            .learn_response_turn(operation, "business-later")
            .unwrap();
        assert_eq!(
            store.contexts.turn_token(operation).as_deref(),
            expected.or(Some("business-later"))
        );
        response_context(&store, &first)
            .translate_value(
                json!({"type":"response.completed","response":{"id":"resp_first","output":[]}}),
            )
            .await
            .unwrap();
        let mut next = request(json!([input("next business")]));
        next["client_metadata"] = json!({"session_id":"startup","turn_id":"turn-two"});
        let next = websocket_request(
            &store,
            next,
            selected_socket,
            first.resolved_identity.as_ref(),
        )
        .await
        .unwrap();
        assert!(next.headers.get("x-codex-turn-state").is_none());
        assert!(
            store
                .contexts
                .turn_token(next.operation.as_ref().unwrap())
                .is_none()
        );
    }
}

#[tokio::test]
async fn invalid_compaction_output_never_commits_identity_or_context_windows() {
    for mode in [
        "empty",
        "assistant",
        "two",
        "missing-encryption",
        "duplicate-event",
        "out-of-range-duplicate",
        "changed-final",
        "valid",
        "stream-only",
    ] {
        let (_temp, store) = store();
        let mut body = request(json!([{"type":"compaction_trigger"}]));
        let metadata = json!({"session_id":"compact","thread_id":"compact","turn_id":"compact-one",
            "request_kind":"compaction","compaction":{"implementation":"responses_compaction_v2"}});
        body["client_metadata"] = json!({"x-codex-turn-metadata":metadata.to_string()});
        let prepared = prepare(&store, body).await.unwrap();
        let identity = prepared.resolved_identity.as_ref().unwrap().clone();
        let response = response_context(&store, &prepared);
        let created = response
            .translate_value(json!({"type":"response.created","response":{"id":"resp_compact"}}))
            .await
            .unwrap();
        let compacted = json!({"type":"compaction","encrypted_content":"opaque-state"});
        let output = match mode {
            "empty" => json!([]),
            "assistant" => {
                json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"wrong V2 summary"}]}])
            }
            "two" => json!([compacted.clone(), compacted.clone()]),
            "missing-encryption" => json!([{"type":"compaction"}]),
            _ => json!([compacted.clone()]),
        };
        for (index, item) in output.as_array().unwrap().iter().enumerate() {
            let index = if mode == "out-of-range-duplicate" {
                store.contexts.limits.output_items + 1
            } else {
                index
            };
            let done = json!({"type":"response.output_item.done","output_index":index,"item":item});
            response.translate_value(done.clone()).await.unwrap();
            if matches!(mode, "duplicate-event" | "out-of-range-duplicate") {
                response.translate_value(done).await.unwrap();
            }
        }
        let mut terminal =
            json!({"type":"response.completed","response":{"id":"resp_compact","output":output}});
        if mode == "stream-only" {
            terminal["response"]
                .as_object_mut()
                .unwrap()
                .remove("output");
        }
        if mode == "changed-final" {
            terminal["response"]["output"][0]["encrypted_content"] = "changed-state".into();
        }
        let completed = response.translate_value(terminal).await;
        if mode == "changed-final" {
            assert!(
                completed.is_err(),
                "conflicting footer must fail before delivery"
            );
        } else {
            assert!(completed.is_ok());
        }
        let accepted = matches!(mode, "valid" | "stream-only");
        let thread = identity.thread_id.clone();
        let window = store
            .edit(NAMESPACE, OWNER, KEY, move |editor| {
                Ok(editor.window_number(&thread).unwrap())
            })
            .await
            .unwrap();
        assert_eq!(window, u64::from(accepted), "identity window: {mode}");
        let inner = store.contexts.inner.lock().unwrap();
        let scope = &inner.scopes[&ContextStore::scope_key(NAMESPACE, KEY)];
        let record = scope
            .records
            .get(created["response"]["id"].as_str().unwrap());
        assert_eq!(record.is_some(), accepted, "context eligibility: {mode}");
        if let Some(record) = record {
            assert_eq!(record.identity.window_number, 1);
        }
    }
}

#[path = "subscription_lineage_tests.rs"]
mod lineage_tests;

#[path = "subscription_control_history_tests.rs"]
mod control_history_tests;
