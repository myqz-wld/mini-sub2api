use super::*;
use crate::subscription_index::Interner;

#[tokio::test]
async fn restoring_hidden_reasoning_checks_source_thread_even_without_item_turn_metadata() {
    for target in ["sibling", "same", "fork"] {
        let (_temp, store) = store();
        let mut body = request(json!([input("seed")]));
        body["include"] = json!([]);
        body["client_metadata"] = json!({"session_id":"root","thread_id":"child-a","parent_thread_id":"root","turn_id":"turn-a"});
        let first = prepare(&store, body.clone()).await.unwrap();
        let response = publish(
            &store,
            first,
            "resp_lineage_cipher",
            json!([reasoning("rs_lineage", "private-child-a")]),
        )
        .await;
        assert!(
            response["output"][0]
                .get("internal_chat_message_metadata_passthrough")
                .is_none()
        );
        body["input"] = json!([input("seed"), response["output"][0], input("next")]);
        body["client_metadata"] = json!({"session_id":"root","thread_id":if target=="same" {"child-a"} else {"child-b"},"parent_thread_id":"root","turn_id":"turn-next"});
        if target == "fork" {
            body["client_metadata"]["forked_from_thread_id"] = json!("child-a");
        }
        let prepared = prepare(&store, body).await;
        if target == "sibling" {
            assert!(
                matches!(prepared, Err(StatefulPrepareError::InvalidRequest)),
                "hidden reasoning was restored into an unrelated sibling thread"
            );
        } else {
            let wire: Value = serde_json::from_slice(&prepared.unwrap().body).unwrap();
            assert!(wire["input"][1]["encrypted_content"] == "private-child-a");
        }
    }
}

#[test]
fn coarse_reasoning_keys_never_merge_distinct_private_contexts() {
    let mut interner = Interner::default();
    let first = interner.intern(reasoning("rs_same", "cipher-a"));
    let second = interner.intern(reasoning("rs_same", "cipher-b"));
    assert!(
        first.lookup_key.id == second.lookup_key.id,
        "coarse lookup did not group redacted candidates"
    );
    assert!(
        first.key.id != second.key.id,
        "private context equality ignored ciphertext"
    );
    assert!(
        !std::sync::Arc::ptr_eq(&first, &second),
        "different ciphertext shared stored content"
    );
}

#[tokio::test]
async fn conflicting_hidden_contexts_reject_and_explicit_session_disambiguates() {
    let (_temp, store) = store();
    let mut body = request(json!([input("same seed")]));
    body["include"] = json!([]);
    let first = prepare(&store, body.clone()).await.unwrap();
    let session = first.resolved_identity.as_ref().unwrap().session_id.clone();
    let second = prepare(&store, body.clone()).await.unwrap();
    let first = publish(
        &store,
        first,
        "resp_hidden_a",
        json!([reasoning("rs_same", "cipher-a")]),
    )
    .await;
    publish(
        &store,
        second,
        "resp_hidden_b",
        json!([reasoning("rs_same", "cipher-b")]),
    )
    .await;
    body["input"] = json!([input("same seed"), first["output"][0], input("next")]);
    assert!(
        matches!(
            prepare(&store, body.clone()).await,
            Err(StatefulPrepareError::StateUnavailable)
        ),
        "ambiguous hidden history selected one context"
    );
    body["client_metadata"] = json!({"session_id":session});
    let chosen = prepare(&store, body).await.unwrap();
    let wire: Value = serde_json::from_slice(&chosen.body).unwrap();
    assert!(
        wire["input"][1]["encrypted_content"] == "cipher-a",
        "explicit session restored another ciphertext"
    );
}

#[tokio::test]
async fn redaction_permission_requires_the_returned_history_and_preserves_supplied_ciphertext() {
    for visible in [false, true] {
        for change in ["missing", "null", "cipher", "summary", "id", "drop-item"] {
            let (_temp, store) = store();
            let mut body = request(json!([input("seed")]));
            body["include"] = if visible {
                json!(["reasoning.encrypted_content"])
            } else {
                json!([])
            };
            let prepared = prepare(&store, body.clone()).await.unwrap();
            let session = prepared
                .resolved_identity
                .as_ref()
                .unwrap()
                .session_id
                .clone();
            let response = publish(
                &store,
                prepared,
                "resp_hidden_edit",
                json!([reasoning("rs_edit", "cipher-original")]),
            )
            .await;
            let mut item = response["output"][0].clone();
            match change {
                "missing" => {
                    item.as_object_mut().unwrap().remove("encrypted_content");
                }
                "null" => item["encrypted_content"] = Value::Null,
                "cipher" => item["encrypted_content"] = json!("cipher-caller"),
                "summary" => item["summary"] = json!([]),
                "id" => item["id"] = json!("rs_unrelated"),
                _ => {}
            }
            body["input"] = if change == "drop-item" {
                json!([input("seed"), input("next")])
            } else {
                json!([input("seed"), item, input("next")])
            };
            let prepared = prepare(&store, body).await.unwrap();
            let should_restore = !visible && matches!(change, "missing" | "null");
            assert!(
                (prepared.resolved_identity.as_ref().unwrap().session_id == session)
                    == should_restore,
                "history edit unexpectedly restored an identity"
            );
            let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
            if should_restore {
                assert!(wire["input"][1]["encrypted_content"] == "cipher-original");
            } else if change == "cipher" {
                assert!(wire["input"][1]["encrypted_content"] == "cipher-caller");
            } else if !visible {
                assert!(
                    !wire["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|item| item["encrypted_content"] == "cipher-original")
                );
            }
        }
    }
}

#[tokio::test]
async fn hidden_provenance_does_not_survive_history_expiry_or_restore_across_keys() {
    let (_temp, store) = store();
    let mut body = request(json!([input("seed")]));
    body["include"] = json!([]);
    let prepared = prepare(&store, body.clone()).await.unwrap();
    let session = prepared
        .resolved_identity
        .as_ref()
        .unwrap()
        .session_id
        .clone();
    let response = publish(
        &store,
        prepared,
        "resp_hidden_expiry",
        json!([reasoning("rs_expiry", "cipher-expiry")]),
    )
    .await;
    body["input"] = json!([input("seed"), response["output"][0], input("next")]);
    let other = prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription1560,
        EmulationTransport::Http,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        128 * 1024 * 1024,
        CodexStateContext {
            force_lite: false,
            admission: None,
            store: &store,
            state_namespace: NAMESPACE,
            account_ref: OWNER,
            downstream_scope: "different-key",
            fingerprint_mode: FingerprintMode::Device,
            binding: None,
            socket_id: None,
        },
        false,
    )
    .await
    .unwrap();
    assert!(other.resolved_identity.as_ref().unwrap().session_id != session);
    let wire: Value = serde_json::from_slice(&other.body).unwrap();
    assert!(
        !wire["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["encrypted_content"] == "cipher-expiry")
    );
    drop(other);
    store.contexts.inner.lock().unwrap().ttl = std::time::Duration::ZERO;
    let delta =
        json!({"model":"gpt-5.5","include":[],"previous_response_id":response["id"],"input":[]});
    assert!(matches!(
        prepare(&store, delta).await,
        Err(StatefulPrepareError::StateUnavailable)
    ));
    let full = prepare(&store, body).await.unwrap();
    let wire: Value = serde_json::from_slice(&full.body).unwrap();
    assert!(
        !wire["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["encrypted_content"] == "cipher-expiry")
    );
}
