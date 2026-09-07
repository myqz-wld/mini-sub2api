use super::*;

#[path = "subscription_checkpoint_boundary_tests.rs"]
mod boundary_tests;

fn anonymous_compaction(lite: bool) -> Value {
    let mut body = compact_request(
        vec![message("developer", "old environment"), input("old user")],
        lite,
    );
    body["client_metadata"] = json!({"x-codex-turn-metadata":
        json!({"request_kind":"compaction","compaction":
            {"implementation":"responses_compaction_v2"}}).to_string()});
    body
}

fn replacement(response: &Value, lite: bool) -> Value {
    let mut items = vec![
        message("developer", "new environment"),
        response["output"][0].clone(),
        input("new user"),
    ];
    if lite {
        items.insert(
            0,
            json!({"type":"additional_tools","role":"developer","tools":[]}),
        );
    }
    let mut body = request(json!(items));
    body["instructions"] = "current base".into();
    body
}

#[tokio::test]
async fn checkpoint_reorganization_keeps_anonymous_identity_but_uses_current_context() {
    for format in 0..3 {
        let (_temp, store) = store();
        let mut body = anonymous_compaction(format == 2);
        if format == 1 {
            body["model"] = "gpt-5.6-sol".into();
        }
        let first = prepare(&store, body).await.unwrap();
        let owner = first.resolved_identity.as_ref().unwrap().clone();
        let completed = finish(&store, first, vec![compacted("private checkpoint")], true).await;
        let mut current = replacement(&completed, format == 2);
        if format == 1 {
            current["model"] = "gpt-5.6-sol".into();
        }
        let next = prepare(&store, current).await.unwrap();
        let identity = next.resolved_identity.as_ref().unwrap();
        assert!(
            identity.session_id == owner.session_id,
            "compaction lost anonymous session"
        );
        assert!(
            identity.thread_id == owner.thread_id,
            "compaction lost thread"
        );
        assert_eq!(identity.window_number, 1);
        assert!(
            identity.turn_id != owner.turn_id,
            "checkpoint restored old turn"
        );
        let wire: Value = serde_json::from_slice(&next.body).unwrap();
        assert!(wire.get("previous_response_id").is_none());
        let encoded = wire.to_string();
        assert!(encoded.contains("new environment") && encoded.contains("new user"));
        assert!(!encoded.contains("old environment") && !encoded.contains("old user"));
        assert!(encoded.contains("current base"));
        let response = publish(&store, next, "resp_after_checkpoint", json!([])).await;
        let cached = history(&store, &response).unwrap().values();
        assert!(cached.iter().any(|item| item["type"] == "compaction"));
        assert!(!serde_json::to_string(&cached).unwrap().contains("old user"));
        let third = prepare(&store, delta(&response, vec![input("later user")]))
            .await
            .unwrap();
        assert!(third.resolved_identity.as_ref().unwrap().session_id == owner.session_id);
    }
}

#[tokio::test]
async fn a_verified_checkpoint_only_window_is_a_completed_prefix() {
    let (_temp, store) = store();
    let first = prepare(&store, request(json!([input("source")])))
        .await
        .unwrap();
    let owner = first.resolved_identity.as_ref().unwrap().session_id.clone();
    let completed = finish(&store, first, vec![compacted("only checkpoint")], true).await;
    let full = request(json!([completed["output"][0], input("next")]));
    let object = full.as_object().unwrap();
    let evidence = Evidence::read(object, &HeaderMap::new(), EmulationTransport::Http).unwrap();
    let plan = store
        .contexts
        .plan(
            ContextStore::scope_key(NAMESPACE, KEY),
            object,
            evidence,
            None,
            None,
        )
        .unwrap();
    assert!(
        plan.baseline.is_some(),
        "verified checkpoint omitted from prefix index"
    );
    assert!(plan.session.as_deref() == Some(owner.as_str()));
}

#[tokio::test]
async fn checkpoint_association_does_not_restore_discarded_tool_dependencies() {
    let (_temp, store) = store();
    let mut body = anonymous_compaction(false);
    body["input"].as_array_mut().unwrap().insert(1,
        json!({"type":"function_call","id":"fc_old","call_id":"call_old","name":"probe","arguments":"{}"}));
    body["input"].as_array_mut().unwrap().insert(
        2,
        json!({"type":"function_call_output","call_id":"call_old","output":"closed"}),
    );
    let first = prepare(&store, body).await.unwrap();
    let completed = finish(&store, first, vec![compacted("checkpoint")], true).await;
    for item in [
        json!({"type":"function_call_output","call_id":"call_old","output":"again"}),
        json!({"type":"item_reference","id":"fc_old"}),
    ] {
        let mut full = replacement(&completed, false);
        full["input"].as_array_mut().unwrap().push(item);
        assert!(
            prepare(&store, full).await.is_err(),
            "checkpoint revived discarded dependency"
        );
    }
}

#[tokio::test]
async fn caller_supplied_checkpoint_is_not_gateway_provenance() {
    let (_temp, store) = store();
    let first = prepare(
        &store,
        request(json!([input("external source"), compacted("external")])),
    )
    .await
    .unwrap();
    let owner = first.resolved_identity.as_ref().unwrap().session_id.clone();
    publish(&store, first, "resp_external", json!([])).await;
    let current = request(json!([
        message("developer", "different prefix"),
        compacted("external"),
        input("next")
    ]));
    let next = prepare(&store, current).await.unwrap();
    assert!(next.resolved_identity.as_ref().unwrap().session_id != owner);
}
