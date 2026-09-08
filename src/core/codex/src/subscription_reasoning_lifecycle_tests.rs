use super::*;

#[tokio::test]
async fn hidden_reasoning_hydration_respects_assembly_capacity() {
    let (_temp, mut store) = store();
    let mut limits = InferenceLimits::load().unwrap();
    limits.global_bytes = 2 * 1024 * 1024;
    limits.key_bytes = 1024 * 1024;
    limits.session_bytes = 512 * 1024;
    limits.output_bytes = 128 * 1024;
    limits.output_items = 4;
    store.contexts = ContextStore::new(limits);
    let mut body = request(json!([input("small public input")]));
    body["include"] = json!([]);
    let first = prepare(&store, body.clone()).await.unwrap();
    let response = publish(
        &store,
        first,
        "resp_large_hidden",
        json!([reasoning("rs_large_hidden", &"x".repeat(70 * 1024))]),
    )
    .await;
    {
        let inner = store.contexts.inner.lock().unwrap();
        let record = &inner.scopes[&ContextStore::scope_key(NAMESPACE, KEY)].records
            [response["id"].as_str().unwrap()];
        assert!(
            record.history.is_some(),
            "large ciphertext fixture was not retained"
        );
    }
    body["input"] = json!([
        input("small public input"),
        response["output"][0],
        input("next")
    ]);
    assert!(
        matches!(
            prepare(&store, body).await,
            Err(StatefulPrepareError::StateUnavailable)
        ),
        "small public input bypassed private assembly capacity"
    );
    assert!(store.contexts.inner.lock().unwrap().operations.is_empty());
}

#[tokio::test]
async fn ineligible_longer_redacted_candidate_cannot_veto_eligible_shorter_history() {
    let (_temp, store) = store();
    let mut body = request(json!([input("seed")]));
    body["include"] = json!([]);
    let prepared = prepare(&store, body).await.unwrap();
    let first = publish(
        &store,
        prepared,
        "resp_short_hidden",
        json!([reasoning("rs_short", "cipher-short")]),
    )
    .await;
    let next =
        json!({"model":"gpt-5.5","previous_response_id":first["id"],"input":[input("second")]});
    let prepared = prepare(&store, next).await.unwrap();
    let mut second = publish(
        &store,
        prepared,
        "resp_long_visible",
        json!([reasoning("rs_long", "cipher-long")]),
    )
    .await;
    second["output"][0]
        .as_object_mut()
        .unwrap()
        .remove("encrypted_content");
    let body = json!({"model":"gpt-5.5","input":[input("seed"),first["output"][0],input("second"),second["output"][0],input("third")]});
    let object = body.as_object().unwrap();
    let plan = store
        .contexts
        .plan(
            ContextStore::scope_key(NAMESPACE, KEY),
            object,
            Evidence::read(object, &HeaderMap::new(), EmulationTransport::Http).unwrap(),
            None,
            None,
        )
        .unwrap();
    assert!(
        plan.baseline
            .as_ref()
            .unwrap()
            .history
            .as_ref()
            .unwrap()
            .len
            == 2,
        "ineligible longer ciphertext record vetoed the shorter history"
    );
    assert!(plan.input()[1]["encrypted_content"] == "cipher-short");
    assert!(
        plan.input()[3].get("encrypted_content").is_none(),
        "visible ciphertext omission was treated as gateway redaction"
    );
    assert!(
        plan.evidence.input[1].get("encrypted_content").is_none(),
        "original caller evidence was rewritten"
    );
}

#[tokio::test]
async fn compaction_keeps_its_ciphertext_and_discards_hidden_reasoning_provenance() {
    for model in ["gpt-5.5", "gpt-5.6-sol"] {
        let (_temp, store) = store();
        let body = json!({"model":model,"include":[],"input":[input("seed")]});
        let prepared = prepare(&store, body).await.unwrap();
        let first = publish(
            &store,
            prepared,
            "resp_compact_hidden",
            json!([reasoning("rs_old", "cipher-old")]),
        )
        .await;
        let body = json!({"model":model,"include":[],"previous_response_id":first["id"],"input":[{"type":"compaction_trigger"}]});
        let prepared = prepare(&store, body).await.unwrap();
        let context = ResponseStateContext::new(
            OWNER,
            NAMESPACE,
            KEY,
            &store,
            prepared.resolved_identity.as_ref(),
            prepared.pending_compaction.as_ref(),
        )
        .with_operation(prepared.operation);
        let checkpoint =
            json!({"type":"compaction","id":"cmp_hidden","encrypted_content":"checkpoint"});
        context
            .translate_value(json!({"type":"response.created","response":{"id":"resp_checkpoint"}}))
            .await
            .unwrap();
        let done = context
            .translate_value(
                json!({"type":"response.output_item.done","output_index":0,"item":checkpoint}),
            )
            .await
            .unwrap();
        assert!(done["item"]["encrypted_content"] == "checkpoint");
        let done = context.translate_value(json!({"type":"response.completed","response":{"id":"resp_checkpoint","output":[checkpoint]}})).await.unwrap();
        let id = done["response"]["id"].as_str().unwrap();
        {
            let inner = store.contexts.inner.lock().unwrap();
            let history = inner.scopes[&ContextStore::scope_key(NAMESPACE, KEY)].records[id]
                .history
                .as_ref()
                .unwrap();
            assert!(
                history.hidden_reasoning().is_empty(),
                "compaction retained discarded reasoning provenance"
            );
        }
        let next = prepare(&store,json!({"model":model,"include":[],"previous_response_id":id,"input":[input("after compact")]})).await.unwrap();
        let wire: Value = serde_json::from_slice(&next.body).unwrap();
        assert!(
            wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["encrypted_content"] == "checkpoint")
        );
        assert!(
            !wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["encrypted_content"] == "cipher-old")
        );
    }
}

async fn prepare_ws(
    store: &RequestStateStore,
    body: Value,
    socket: &str,
) -> PreparedEmulatedRequest {
    prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription1534,
        EmulationTransport::WebSocket,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        128 * 1024 * 1024,
        CodexStateContext {
            account_ref: OWNER,
            state_namespace: NAMESPACE,
            downstream_scope: KEY,
            fingerprint_mode: FingerprintMode::Device,
            store,
            binding: None,
            force_lite: false,
            admission: None,
            socket_id: Some(socket),
        },
        false,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn live_ws_keeps_return_policy_without_recreating_expired_ciphertext_history() {
    let (_temp, store) = store();
    let socket = store.contexts.open_socket().unwrap();
    let prepared = prepare_ws(
        &store,
        json!({"type":"response.create","model":"gpt-5.5","include":[],"input":[input("seed")]}),
        &socket.id,
    )
    .await;
    let first = publish(
        &store,
        prepared,
        "resp_live_hidden",
        json!([reasoning("rs_live_first", "cipher-first")]),
    )
    .await;
    store.contexts.inner.lock().unwrap().ttl = std::time::Duration::ZERO;
    let prepared = prepare_ws(&store,json!({"type":"response.create","model":"gpt-5.5","include":[],"previous_response_id":first["id"],"input":[input("second")]}),&socket.id).await;
    let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
    assert!(wire["previous_response_id"] == "resp_live_hidden");
    let second = publish(
        &store,
        prepared,
        "resp_live_second",
        json!([reasoning("rs_live_second", "cipher-second")]),
    )
    .await;
    assert!(second["output"][0].get("encrypted_content").is_none());
    let inner = store.contexts.inner.lock().unwrap();
    let record = &inner.scopes[&ContextStore::scope_key(NAMESPACE, KEY)].records
        [second["id"].as_str().unwrap()];
    assert!(
        record.history.is_none(),
        "remote-only WS manufactured complete private history"
    );
}
