use super::*;

fn body() -> Value {
    request(json!([
        {"type":"message","role":"developer","content":"synthetic setup"},
        input("synthetic original task")
    ]))
}

async fn seed(
    store: &RequestStateStore,
    response_id: &str,
    with_alias: bool,
) -> (
    crate::request_identity_projection::ResolvedRequestIdentity,
    Value,
) {
    let prepared = prepare(store, body()).await.unwrap();
    let identity = prepared.resolved_identity.as_ref().unwrap().clone();
    let mut output = json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic answer"}]});
    if with_alias {
        output["id"] = "msg_synthetic_shared".into();
    }
    let response = publish(store, prepared, response_id, json!([output])).await;
    let mut replay = body();
    replay["input"][0]["content"] = "synthetic changed setup".into();
    replay["input"]
        .as_array_mut()
        .unwrap()
        .extend([response["output"][0].clone(), input("synthetic next task")]);
    (identity, replay)
}

#[tokio::test]
async fn alias_association_does_not_grant_a_baseline_or_rewrite_caller_setup() {
    let (_temp, store) = store();
    let (source, replay) = seed(&store, "resp_alias", true).await;
    let evidence = Evidence::read(
        replay.as_object().unwrap(),
        &HeaderMap::new(),
        EmulationTransport::Http,
    )
    .unwrap();
    let plan = store
        .contexts
        .plan(
            ContextStore::scope_key(NAMESPACE, KEY),
            replay.as_object().unwrap(),
            evidence,
            None,
            None,
        )
        .unwrap();
    assert!(plan.baseline.is_none());
    assert!(plan.association.is_some());
    drop(plan);
    let prepared = prepare(&store, replay).await.unwrap();
    assert_eq!(
        prepared.resolved_identity.as_ref().unwrap().session_id,
        source.session_id
    );
    let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
    assert!(wire.get("previous_response_id").is_none());
    assert!(
        wire["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["content"][0]["text"] == "synthetic changed setup")
    );
}

#[tokio::test]
async fn alias_association_requires_aliases_and_does_not_accept_changed_business_history() {
    for case in ["no-alias", "changed-task", "changed-output", "expired"] {
        let (_temp, store) = store();
        let (source, mut replay) = seed(&store, "resp_alias_bound", case != "no-alias").await;
        match case {
            "changed-task" => replay["input"][1] = input("synthetic different past task"),
            "changed-output" => {
                replay["input"][2]["content"][0]["text"] = "synthetic different answer".into()
            }
            "expired" => store
                .contexts
                .inner
                .lock()
                .unwrap()
                .expire(std::time::Instant::now() + std::time::Duration::from_secs(4 * 60 * 60)),
            _ => {}
        }
        let prepared = prepare(&store, replay).await.unwrap();
        assert_ne!(
            prepared.resolved_identity.as_ref().unwrap().session_id,
            source.session_id,
            "{case}"
        );
    }
}

#[tokio::test]
async fn shared_alias_cannot_resolve_two_anonymous_owners_by_recency() {
    let (_temp, store) = store();
    let (first, replay) = seed(&store, "resp_alias_first", true).await;
    let (second, _) = seed(&store, "resp_alias_second", true).await;
    assert_ne!(first.session_id, second.session_id);
    assert!(matches!(
        prepare(&store, replay).await,
        Err(StatefulPrepareError::StateUnavailable)
    ));
}

#[tokio::test]
async fn explicit_session_and_other_key_do_not_use_anonymous_alias_association() {
    let (_temp, store) = store();
    let (source, replay) = seed(&store, "resp_alias_scope", true).await;
    for explicit in [false, true] {
        let mut replay = replay.clone();
        if explicit {
            replay["client_metadata"] = json!({"session_id":"synthetic-explicit-other"});
        }
        let prepared = prepare_stateful_codex_request(
            UpstreamProfile::CodexSubscription1620,
            EmulationTransport::Http,
            &HeaderMap::new(),
            Bytes::from(serde_json::to_vec(&replay).unwrap()),
            128 * 1024 * 1024,
            CodexStateContext {
                store: &store,
                state_namespace: NAMESPACE,
                account_ref: OWNER,
                downstream_scope: if explicit { KEY } else { "synthetic-other-key" },
                fingerprint_mode: FingerprintMode::Device,
                binding: None,
                socket_id: None,
                force_lite: false,
                admission: None,
            },
            false,
        )
        .await
        .unwrap();
        assert_ne!(
            prepared.resolved_identity.as_ref().unwrap().session_id,
            source.session_id
        );
    }
}
