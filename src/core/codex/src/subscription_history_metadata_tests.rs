use super::*;

#[tokio::test]
async fn omitted_assistant_metadata_retains_history_identity_and_explicit_conflicts_do_not() {
    for model in ["gpt-5.4", "gpt-6-astra"] {
        for keep_id in [false, true] {
            let (_temp, store) = store();
            let mut first = request(json!([input("first")]));
            first["model"] = model.into();
            let prepared = prepare(&store, first.clone()).await.unwrap();
            let identity = prepared.resolved_identity.as_ref().unwrap().clone();
            let mut output = assistant();
            output["metadata"] = json!({"provider_annotation":"synthetic"});
            let response = publish(&store, prepared, "resp_metadata", json!([output])).await;
            let mut copied = response["output"][0].clone();
            if !keep_id {
                copied = caller_copy(copied);
            }
            let mut next = first;
            next["input"] = json!([input("first"), copied.clone(), input("second")]);
            assert!(plan(&store, &next, KEY).unwrap().baseline.is_some());
            next["input"][1]["metadata"] = json!({"provider_annotation":"changed"});
            assert!(plan(&store, &next, KEY).unwrap().baseline.is_none());
            copied.as_object_mut().unwrap().remove("metadata");
            next["input"][1] = copied;
            assert!(plan(&store, &next, KEY).unwrap().baseline.is_some());
            assert!(plan(&store, &next, "other-key").unwrap().baseline.is_none());
            let prepared = prepare(&store, next).await.unwrap();
            let resumed = prepared.resolved_identity.as_ref().unwrap();
            assert_eq!(resumed.session_id, identity.session_id);
            assert_eq!(resumed.thread_id, identity.thread_id);
            assert_ne!(resumed.turn_id, identity.turn_id);
        }
    }
}

#[tokio::test]
async fn omitted_metadata_cannot_choose_between_different_stored_histories() {
    let (_temp, store) = store();
    let first = request(json!([input("first")]));
    for annotation in ["one", "two"] {
        let prepared = prepare(&store, first.clone()).await.unwrap();
        let mut output = assistant();
        output["id"] = format!("msg_{annotation}").into();
        output["metadata"] = json!({"provider_annotation":annotation});
        publish(
            &store,
            prepared,
            &format!("resp_{annotation}"),
            json!([output]),
        )
        .await;
    }
    let mut next = first;
    next["input"] = json!([input("first"), caller_copy(assistant()), input("second")]);
    assert!(matches!(
        plan(&store, &next, KEY),
        Err(StatefulPrepareError::StateUnavailable)
    ));
    next["input"][1]["metadata"] = json!({"provider_annotation":"one"});
    assert!(plan(&store, &next, KEY).unwrap().baseline.is_some());
}

#[test]
fn metadata_projection_preserves_exact_keys_completion_checks_and_business_fields() {
    use crate::subscription_index::{Interner, completion_items_compatible, metadata_compatible};
    let mut interner = Interner::default();
    let mut full = assistant();
    full["metadata"] = json!({"server":"annotation"});
    let mut omitted = full.clone();
    omitted.as_object_mut().unwrap().remove("metadata");
    let a = interner.intern(full.clone());
    let b = interner.intern(omitted.clone());
    assert_ne!(a.key.id, b.key.id);
    assert_eq!(a.lookup_key.id, b.lookup_key.id);
    assert!(metadata_compatible(&omitted, &full));
    assert!(!metadata_compatible(&full, &omitted));
    assert!(!completion_items_compatible(&full, &omitted));
    for mut opaque in [
        json!({"type":"message","role":"user","content":"user data"}),
        json!({"type":"function_call_output","call_id":"call_test","output":"tool data"}),
    ] {
        let bare = interner.intern(opaque.clone());
        opaque["metadata"] = json!({"business":"value"});
        let decorated = interner.intern(opaque);
        assert_ne!(bare.lookup_key.id, decorated.lookup_key.id);
    }
    let mut nested = omitted;
    nested["content"][0]["metadata"] = json!({"business":"value"});
    assert_ne!(interner.intern(nested).lookup_key.id, b.lookup_key.id);
}
