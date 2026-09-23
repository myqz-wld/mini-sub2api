use super::*;

fn branch_request(
    session: &str,
    thread: &str,
    turn: &str,
    parent_turn: Option<&str>,
    history: &[&str],
) -> Value {
    let mut metadata = serde_json::json!({"session_id":session,"thread_id":thread,"turn_id":turn});
    if let Some(parent) = parent_turn {
        metadata["parent_thread_id"] = session.into();
        metadata["parent_turn_id"] = parent.into();
        metadata["root_turn_id"] = parent.into();
    }
    let input: Vec<_> = history.iter().copied().chain([turn]).map(|turn|
        serde_json::json!({"type":"message","id":format!("msg_{turn}"),"role":"user","content":turn,
            "internal_chat_message_metadata_passthrough":{"turn_id":turn}})).collect();
    serde_json::json!({"model":"gpt-5.4","instructions":"base","input":input,"client_metadata":metadata})
}

async fn attempt(
    store: &RequestStateStore,
    body: Value,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    prepare_identity_request(
        UpstreamProfile::CodexSubscription1560,
        EmulationTransport::Http,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        1024 * 1024,
        CodexStateContext {
            force_lite: false,
            admission: None,
            binding: None,
            socket_id: None,
            account_ref: ACCOUNT_REF,
            state_namespace: NAMESPACE,
            downstream_scope: SCOPE,
            fingerprint_mode: FingerprintMode::Device,
            store,
        },
        false,
    )
    .await
}

#[tokio::test]
async fn historical_attribution_retains_its_owner_and_causal_parent_with_or_without_item_ids() {
    for lite in [false, true] {
        for with_ids in [false, true] {
            let (_temp, store) = store();
            let root = prepare(
                &store,
                &HeaderMap::new(),
                branch_request("root", "root", "p1", None, &[]),
            )
            .await;
            let child = prepare(
                &store,
                &HeaderMap::new(),
                branch_request("root", "child", "c1", Some("p1"), &[]),
            )
            .await;
            prepare(
                &store,
                &HeaderMap::new(),
                branch_request("root", "root", "p2", None, &[]),
            )
            .await;
            let mut body = branch_request("root", "child", "c2", Some("p2"), &["p1", "c1"]);
            if lite {
                body["model"] = "gpt-5.6-sol".into();
            }
            if !with_ids {
                for item in body["input"].as_array_mut().unwrap() {
                    item.as_object_mut().unwrap().remove("id");
                }
            }
            let prepared = attempt(&store, body).await.unwrap();
            let data = value(&prepared);
            let turns: Vec<_> = data["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|item| {
                    item["internal_chat_message_metadata_passthrough"]["turn_id"].as_str()
                })
                .collect();
            let root_identity = root.resolved_identity.unwrap();
            let child_identity = child.resolved_identity.unwrap();
            assert!(turns.contains(&root_identity.turn_id.as_deref().unwrap()));
            assert!(turns.contains(&child_identity.turn_id.as_deref().unwrap()));
            assert_eq!(
                turns.last().copied(),
                prepared.resolved_identity.unwrap().turn_id.as_deref()
            );
            let saved = store
                .edit(NAMESPACE, ACCOUNT_REF, SCOPE, move |editor| {
                    let key = editor.lookup("turn", "c1");
                    Ok(editor.existing_turn(&key).unwrap())
                })
                .await
                .unwrap();
            assert_eq!(saved.thread_id, child_identity.thread_id);
            assert_eq!(saved.parent_turn_id, root_identity.turn_id);
        }
    }
}

#[tokio::test]
async fn unrelated_history_and_changed_current_ownership_fail_without_mutating_the_ledger() {
    let (_temp, store) = store();
    for body in [
        branch_request("root", "root", "p1", None, &[]),
        branch_request("root", "child", "c1", Some("p1"), &[]),
        branch_request("other", "other", "foreign", None, &[]),
        branch_request("root", "sibling", "s1", Some("p1"), &[]),
    ] {
        attempt(&store, body).await.unwrap();
    }
    let original = fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
    for body in [
        branch_request("root", "child", "c2", Some("p1"), &["foreign"]),
        branch_request("root", "child", "c2", Some("p1"), &["s1"]),
        branch_request("other", "other", "c1", None, &[]),
        branch_request("other", "child", "c2", Some("foreign"), &[]),
    ] {
        assert!(matches!(
            attempt(&store, body).await,
            Err(StatefulPrepareError::InvalidRequest)
        ));
        assert_eq!(
            fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
            original
        );
    }
    // Supplied fork history can retain its source's attribution; it does not become a current turn.
    let mut fork = branch_request("fork", "fork", "fork-turn", None, &["foreign"]);
    fork["client_metadata"]["forked_from_thread_id"] = "other".into();
    let fork = attempt(&store, fork).await.unwrap();
    let identity = fork.resolved_identity.unwrap();
    assert_eq!(identity.thread_id, identity.session_id);
    assert!(identity.parent_thread_id.is_none());
}

#[tokio::test]
async fn unseen_fork_alias_is_adopted_by_the_actual_root_or_child_after_reopen() {
    for child in [false, true] {
        let (temp, store) = store();
        let mut fork = branch_request("fork", "fork", "fork-turn", None, &[]);
        fork["client_metadata"]["forked_from_thread_id"] = "unseen".into();
        let fork = attempt(&store, fork.clone()).await.unwrap();
        let identity = fork.resolved_identity.unwrap();
        let source = identity.forked_from_thread_id.unwrap();
        assert_uuid_version(&source, 7);
        assert!(identity.parent_thread_id.is_none());
        assert_ne!(source, identity.session_id);
        let reopened = RequestStateStore::new(temp.path().join("accounts"));
        let source_body = if child {
            branch_request(
                "source-root",
                "unseen",
                "source-turn",
                Some("source-parent"),
                &[],
            )
        } else {
            branch_request("unseen", "unseen", "source-turn", None, &[])
        };
        let actual = attempt(&reopened, source_body)
            .await
            .unwrap()
            .resolved_identity
            .unwrap();
        assert_eq!(actual.thread_id, source);
        assert_eq!(actual.parent_thread_id.is_some(), child);
    }
}

#[tokio::test]
async fn unseen_historical_turn_does_not_acquire_the_fork_threads_ownership() {
    let (temp, store) = store();
    let mut fork = branch_request("fork", "fork", "fork-turn", None, &["unseen-turn"]);
    fork["client_metadata"]["forked_from_thread_id"] = "source".into();
    let prepared = attempt(&store, fork.clone()).await.unwrap();
    let original_item = value(&prepared)["input"][0].clone();
    let source_turn =
        value(&prepared)["input"][0]["internal_chat_message_metadata_passthrough"]["turn_id"]
            .as_str()
            .unwrap()
            .to_string();
    store
        .edit(NAMESPACE, ACCOUNT_REF, SCOPE, |editor| {
            let key = editor.lookup("turn", "unseen-turn");
            assert!(editor.existing_turn(&key).is_none());
            Ok(())
        })
        .await
        .unwrap();
    let reopened = RequestStateStore::new(temp.path().join("accounts"));
    let actual = attempt(
        &reopened,
        branch_request("source", "source", "unseen-turn", None, &[]),
    )
    .await
    .unwrap();
    assert_eq!(
        actual.resolved_identity.unwrap().turn_id.as_deref(),
        Some(source_turn.as_str())
    );
    let repeated = attempt(&reopened, fork).await.unwrap();
    assert_eq!(value(&repeated)["input"][0], original_item);
}
