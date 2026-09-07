use super::*;

fn plan(
    store: &RequestStateStore,
    namespace: &str,
    key: &str,
    body: &Value,
) -> Result<crate::subscription_prepare::ContextPlan, StatefulPrepareError> {
    let object = body.as_object().unwrap();
    let evidence = Evidence::read(object, &HeaderMap::new(), EmulationTransport::Http).unwrap();
    store.contexts.plan(
        ContextStore::scope_key(namespace, key),
        object,
        evidence,
        None,
        None,
    )
}

#[tokio::test]
async fn checkpoint_association_requires_exact_last_checkpoint_and_anonymous_scope() {
    let (_temp, store) = store();
    let first = prepare(&store, anonymous_compaction(false)).await.unwrap();
    let completed = finish(&store, first, vec![compacted("proven checkpoint")], true).await;
    let full = replacement(&completed, false);
    assert!(
        plan(&store, NAMESPACE, KEY, &full)
            .unwrap()
            .checkpoint
            .is_some()
    );
    for (namespace, key) in [(NAMESPACE, "other-key"), ("other-account", KEY)] {
        assert!(
            plan(&store, namespace, key, &full)
                .unwrap()
                .checkpoint
                .is_none()
        );
    }
    for change in 0..5 {
        let mut changed = full.clone();
        match change {
            0 => changed["input"][1]["encrypted_content"] = "modified ciphertext".into(),
            1 => changed["input"][1]["id"] = "cmp_other".into(),
            2 => {
                changed["input"][1].as_object_mut().unwrap().remove("id");
            }
            3 => changed["input"]
                .as_array_mut()
                .unwrap()
                .push(compacted("newer unknown checkpoint")),
            _ => changed["client_metadata"] = json!({"session_id":"unrelated explicit session"}),
        }
        assert!(
            plan(&store, NAMESPACE, KEY, &changed)
                .unwrap()
                .checkpoint
                .is_none()
        );
    }
    {
        let mut inner = store.contexts.inner.lock().unwrap();
        let scope = inner
            .scopes
            .get_mut(&ContextStore::scope_key(NAMESPACE, KEY))
            .unwrap();
        for session in scope.sessions.values_mut() {
            session.explicit = true;
        }
        scope.rebuild_index();
    }
    assert!(
        plan(&store, NAMESPACE, KEY, &full)
            .unwrap()
            .checkpoint
            .is_none()
    );
}

#[tokio::test]
async fn checkpoint_ownership_conflicts_fail_without_selecting_newest() {
    let (_temp, store) = store();
    let first = prepare(&store, anonymous_compaction(false)).await.unwrap();
    let completed = finish(&store, first, vec![compacted("shared checkpoint")], true).await;
    let full = replacement(&completed, false);
    {
        let mut inner = store.contexts.inner.lock().unwrap();
        let scope = inner
            .scopes
            .get_mut(&ContextStore::scope_key(NAMESPACE, KEY))
            .unwrap();
        let copy = scope.records[completed["id"].as_str().unwrap()].clone();
        scope.records.insert("equivalent response".into(), copy);
        scope.rebuild_index();
    }
    assert!(
        plan(&store, NAMESPACE, KEY, &full)
            .unwrap()
            .checkpoint
            .is_some()
    );
    {
        let mut inner = store.contexts.inner.lock().unwrap();
        let scope = inner
            .scopes
            .get_mut(&ContextStore::scope_key(NAMESPACE, KEY))
            .unwrap();
        let record = scope.records.get_mut("equivalent response").unwrap();
        record.identity.session_id = "other-owner".into();
        record.identity.thread_id = "other-owner".into();
        record.last_used = Instant::now() + Duration::from_secs(1);
        scope.sessions.insert(
            "other-owner".into(),
            crate::subscription_context::Session {
                explicit: false,
                last_business: Instant::now(),
            },
        );
        scope.rebuild_index();
    }
    assert!(matches!(
        plan(&store, NAMESPACE, KEY, &full),
        Err(StatefulPrepareError::StateUnavailable)
    ));
}

#[tokio::test]
async fn checkpoint_index_expires_with_bodies_and_is_not_persisted() {
    let (_temp, store) = store();
    let first = prepare(&store, anonymous_compaction(false)).await.unwrap();
    let completed = finish(
        &store,
        first,
        vec![compacted("secret checkpoint sentinel")],
        true,
    )
    .await;
    let full = replacement(&completed, false);
    assert!(
        plan(&store, NAMESPACE, KEY, &full)
            .unwrap()
            .checkpoint
            .is_some()
    );
    let persisted = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
    assert!(
        !String::from_utf8(persisted)
            .unwrap()
            .contains("secret checkpoint sentinel")
    );
    {
        let mut inner = store.contexts.inner.lock().unwrap();
        let scope = inner
            .scopes
            .get_mut(&ContextStore::scope_key(NAMESPACE, KEY))
            .unwrap();
        assert!(scope.checkpoint_index_cost(None) > 0);
        for session in scope.sessions.values_mut() {
            session.last_business = Instant::now() - Duration::from_secs(3 * 60 * 60 + 1);
        }
    }
    assert!(
        plan(&store, NAMESPACE, KEY, &full)
            .unwrap()
            .checkpoint
            .is_none()
    );
    let inner = store.contexts.inner.lock().unwrap();
    assert!(
        inner
            .scopes
            .values()
            .all(|scope| scope.checkpoints.is_empty())
    );
}

#[tokio::test]
async fn checkpoint_body_eviction_and_rejected_completion_remove_association() {
    for mode in 0..3 {
        let (_temp, store) = store();
        let first = prepare(&store, anonymous_compaction(false)).await.unwrap();
        let completed = finish(&store, first, vec![compacted("checkpoint")], mode != 2).await;
        if mode != 2 {
            let mut inner = store.contexts.inner.lock().unwrap();
            let scope = inner
                .scopes
                .get_mut(&ContextStore::scope_key(NAMESPACE, KEY))
                .unwrap();
            let record = scope
                .records
                .get_mut(completed["id"].as_str().unwrap())
                .unwrap();
            if mode == 0 {
                record.history = None;
                record.settings = None;
            } else {
                record.completed = false;
            }
            scope.rebuild_index();
        }
        assert!(
            plan(&store, NAMESPACE, KEY, &replacement(&completed, false))
                .unwrap()
                .checkpoint
                .is_none()
        );
    }
}

#[tokio::test]
async fn association_preserves_child_identity_and_checks_opaque_history_ownership() {
    let (_temp, store) = store();
    let root = prepare(&store, request(json!([input("root user")])))
        .await
        .unwrap();
    let root_session = root.resolved_identity.as_ref().unwrap().session_id.clone();
    let root_response = publish(&store, root, "resp_root", json!([])).await;
    let mut child = anonymous_compaction(false);
    child["previous_response_id"] = root_response["id"].clone();
    child["client_metadata"]["thread_id"] = "child-source".into();
    child["client_metadata"]["parent_thread_id"] = root_session.clone().into();
    let child = prepare(&store, child).await.unwrap();
    let child_owner = child.resolved_identity.as_ref().unwrap().clone();
    assert!(child_owner.thread_id != child_owner.session_id);
    let completed = finish(&store, child, vec![compacted("child checkpoint")], true).await;
    let full = replacement(&completed, false);
    let next = prepare(&store, full.clone()).await.unwrap();
    assert!(next.resolved_identity.as_ref().unwrap().thread_id == child_owner.thread_id);
    assert!(
        next.resolved_identity.as_ref().unwrap().parent_thread_id == child_owner.parent_thread_id
    );
    drop(next);
    let mut sibling = full;
    sibling["client_metadata"] =
        json!({"thread_id":"child-sibling","parent_thread_id":root_session});
    assert!(
        prepare(&store, sibling).await.is_err(),
        "checkpoint crossed sibling history ownership"
    );
}

#[tokio::test]
async fn checkpoint_identity_does_not_inherit_omitted_ordinary_instructions() {
    let (_temp, store) = store();
    let mut original = anonymous_compaction(true);
    original["input"][0]["tools"] =
        json!([{"type":"function","name":"old_tool","parameters":{"type":"object"}}]);
    let first = prepare(&store, original).await.unwrap();
    let owner = first.resolved_identity.as_ref().unwrap().session_id.clone();
    let completed = finish(&store, first, vec![compacted("checkpoint")], true).await;
    let mut full = replacement(&completed, false);
    full.as_object_mut().unwrap().remove("instructions");
    let next = prepare(&store, full).await.unwrap();
    assert!(next.resolved_identity.as_ref().unwrap().session_id == owner);
    let wire: Value = serde_json::from_slice(&next.body).unwrap();
    assert!(wire.get("instructions").is_none());
    assert!(wire["input"][0]["type"] != "additional_tools");
    assert!(wire.get("tools").is_none());
}

#[tokio::test]
async fn an_unproven_checkpoint_only_request_stays_out_of_the_prefix_index() {
    let (_temp, store) = store();
    let first = prepare(&store, request(json!([compacted("caller checkpoint")])))
        .await
        .unwrap();
    publish(&store, first, "resp_caller_checkpoint", json!([])).await;
    let full = request(json!([compacted("caller checkpoint"), input("new user")]));
    let selected = plan(&store, NAMESPACE, KEY, &full).unwrap();
    assert!(selected.baseline.is_none() && selected.checkpoint.is_none());
}
