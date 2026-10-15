use super::*;

#[tokio::test]
async fn valid_guardian_and_special_http_requests_fork_without_changing_identity() {
    for case in ["reviewer", "classifier", "memory", "prewarm", "compaction"] {
        let (_temp, store) = store();
        let (mut body, _) = tool_history(&store, "gpt-5.4").await;
        let mut headers = HeaderMap::new();
        match case {
            "reviewer" => {
                headers.insert("x-codex-guardian", case.parse().unwrap());
            }
            "classifier" => {
                headers.insert("x-codex-guardian", "classifier".parse().unwrap());
                let source = prepare(&store, json!({"model":"gpt-5.5","input":"synthetic source",
                    "client_metadata":{"session_id":"review-source","thread_id":"review-source","turn_id":"source-turn"}})).await.unwrap();
                publish(&store, source, "resp_review_source", json!([])).await;
                body = json!({"model":"gpt-5.6-luna","input":[
                    {"type":"additional_tools","role":"developer","tools":[]},
                    {"role":"developer","content":"synthetic review"}, input("synthetic evidence")
                ],"client_metadata":{
                    "session_id":"review-source", "thread_id":"synthetic-classifier-lease",
                    "turn_id":"synthetic-classifier-turn", "parent_turn_id":"source-turn",
                    "x-openai-subagent":"guardian", "x-codex-turn-metadata":json!({
                        "session_id":"review-source", "thread_id":"synthetic-classifier-lease",
                        "turn_id":"synthetic-classifier-turn", "parent_turn_id":"source-turn",
                        "guardian_classifier_source_thread_id":"review-source",
                        "thread_source":"guardian_classifier", "turn_trigger":"guardian_classifier"
                    }).to_string()}
                });
            }
            _ => {
                body["client_metadata"] = json!({"x-codex-turn-metadata":
                    json!({"request_kind":case}).to_string()});
                if case == "compaction" {
                    body["input"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"type":"compaction_trigger"}));
                    body["client_metadata"]["x-codex-turn-metadata"] = json!({
                        "request_kind":"compaction", "compaction":{"implementation":"responses_compaction_v2"}
                    }).to_string().into();
                }
            }
        }
        let first = scoped(
            &store,
            body.clone(),
            &headers,
            EmulationTransport::Http,
            None,
            None,
            NAMESPACE,
            KEY,
        )
        .await
        .unwrap_or_else(|err| panic!("{case}: {err}"));
        let second = scoped(
            &store,
            body,
            &headers,
            EmulationTransport::Http,
            None,
            None,
            NAMESPACE,
            KEY,
        )
        .await;
        let second = second.unwrap_or_else(|err| panic!("{case}: {err}"));
        assert_ne!(
            active(&store, &first).branch,
            active(&store, &second).branch,
            "{case}"
        );
        assert_eq!(
            active(&store, &first).identity,
            active(&store, &second).identity,
            "{case}"
        );
        assert_eq!(store.contexts.inner.lock().unwrap().operations.len(), 2);
        drop(first);
    }
}

#[tokio::test]
async fn explicit_context_can_fork_but_one_websocket_still_has_one_active_response() {
    for case in [
        "session",
        "header-session",
        "turn",
        "previous",
        "websocket",
        "binding",
        "lineage",
    ] {
        let (_temp, store) = store();
        let (mut body, seed) = tool_history(&store, "gpt-5.4").await;
        let seed_record = {
            let inner = store.contexts.inner.lock().unwrap();
            inner.scopes[&ContextStore::scope_key(NAMESPACE, KEY)].records
                [seed["id"].as_str().unwrap()]
            .clone()
        };
        let mut headers = HeaderMap::new();
        match case {
            "session" => {
                body["client_metadata"] = json!({"session_id":seed_record.identity.session_id})
            }
            "header-session" => {
                headers.insert(
                    "session-id",
                    seed_record.identity.session_id.parse().unwrap(),
                );
            }
            "turn" => body["client_metadata"] = json!({"turn_id":seed_record.identity.turn_id}),
            "previous" => {
                body["previous_response_id"] = seed["id"].clone();
                body["input"] = json!([body["input"][2]]);
            }
            "lineage" => {
                body["client_metadata"] = json!({"thread_id":seed_record.identity.thread_id})
            }
            _ => {}
        }
        let transport = if matches!(case, "websocket" | "binding") {
            EmulationTransport::WebSocket
        } else {
            EmulationTransport::Http
        };
        let binding = (case == "binding").then_some(&seed_record.identity);
        let socket =
            matches!(case, "websocket" | "binding").then(|| store.contexts.open_socket().unwrap());
        let socket = socket.as_ref().map(|s| s.id.as_str());
        let first = scoped(
            &store,
            body.clone(),
            &headers,
            transport,
            binding,
            socket,
            NAMESPACE,
            KEY,
        )
        .await
        .unwrap();
        let result = scoped(
            &store, body, &headers, transport, binding, socket, NAMESPACE, KEY,
        )
        .await;
        if matches!(case, "websocket" | "binding") {
            assert!(
                matches!(result, Err(StatefulPrepareError::StateUnavailable)),
                "{case}"
            );
            assert_eq!(store.contexts.inner.lock().unwrap().operations.len(), 1);
        } else {
            let second = result.unwrap_or_else(|err| panic!("{case}: {err}"));
            assert_ne!(
                active(&store, &first).branch,
                active(&store, &second).branch,
                "{case}"
            );
            assert_eq!(
                active(&store, &first).identity,
                active(&store, &second).identity,
                "{case}"
            );
        }
        drop(first);
    }
}

#[tokio::test]
async fn other_keys_and_accounts_keep_independent_context_owners() {
    for (namespace, key) in [(NAMESPACE, "other-key"), ("other-account", KEY)] {
        let (_temp, store) = store();
        let (body, _) = tool_history(&store, "gpt-5.4").await;
        let first = prepare(&store, body.clone()).await.unwrap();
        let second = scoped(
            &store,
            body,
            &HeaderMap::new(),
            EmulationTransport::Http,
            None,
            None,
            namespace,
            key,
        )
        .await
        .unwrap();
        assert_ne!(
            active(&store, &first).identity.session_id,
            active(&store, &second).identity.session_id
        );
        assert_ne!(
            first.operation.as_ref().unwrap().0.scope,
            second.operation.as_ref().unwrap().0.scope
        );
        assert_eq!(store.contexts.inner.lock().unwrap().operations.len(), 2);
    }
}

#[tokio::test]
async fn ordinary_nonconflicting_requests_keep_existing_branch_selection() {
    let (_temp, store) = store();
    let (body, seed) = tool_history(&store, "gpt-5.4").await;
    let baseline = {
        let inner = store.contexts.inner.lock().unwrap();
        inner.scopes[&ContextStore::scope_key(NAMESPACE, KEY)].records[seed["id"].as_str().unwrap()]
            .branch
            .clone()
    };
    let prepared = prepare(&store, body).await.unwrap();
    assert_eq!(active(&store, &prepared).branch, baseline);
    drop(prepared);
    let first = prepare(&store, request(json!([input("synthetic unrelated start")])))
        .await
        .unwrap();
    let second = prepare(&store, request(json!([input("synthetic unrelated start")])))
        .await
        .unwrap();
    assert_ne!(
        active(&store, &first).identity.session_id,
        active(&store, &second).identity.session_id
    );
}
