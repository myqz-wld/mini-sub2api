use super::*;

#[path = "subscription_content_id_reference_tests.rs"]
mod reference_tests;

fn local_history(session: &str, text: &str) -> Value {
    let mut body = request(json!([
        input("Synthetic common opening"),
        {"type":"message","id":"msg_pi_1","role":"assistant",
            "content":[{"type":"output_text","text":text}]},
        {"type":"function_call","id":"fc_pi_2","call_id":"call_pi_1",
            "name":"lookup","arguments":format!("{{\"task\":\"{text}\"}}")},
        {"type":"function_call_output","id":"fco_pi_3","call_id":"call_pi_1","output":"synthetic result"},
        input("Synthetic continuation")
    ]));
    body["client_metadata"] = json!({"session_id":session,"turn_id":format!("turn-{session}")});
    body
}

async fn project(
    store: &RequestStateStore,
    body: Value,
    transport: EmulationTransport,
    namespace: &str,
    key: &str,
) -> PreparedEmulatedRequest {
    prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription1592,
        transport,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        128 * 1024 * 1024,
        CodexStateContext {
            force_lite: false,
            admission: None,
            store,
            state_namespace: namespace,
            account_ref: OWNER,
            downstream_scope: key,
            fingerprint_mode: FingerprintMode::Device,
            binding: None,
            socket_id: None,
        },
        false,
    )
    .await
    .unwrap()
}

fn wire(prepared: &PreparedEmulatedRequest) -> Value {
    serde_json::from_slice(&prepared.body).unwrap()
}

fn identifiers(value: &Value) -> Vec<Value> {
    vec![
        value["input"][1]["id"].clone(),
        value["input"][2]["id"].clone(),
        value["input"][2]["call_id"].clone(),
    ]
}

#[tokio::test]
async fn local_ids_are_content_session_scoped_native_pseudonyms() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        let (_temp, store) = store();
        let original = local_history("session-a", "synthetic task-a");
        let first = project(&store, original.clone(), transport, NAMESPACE, KEY).await;
        let first_wire = wire(&first);
        drop(first);
        let first = first_wire;
        let original_ids = identifiers(&original);
        let first_ids = identifiers(&first);
        for ((id, raw), prefix) in first_ids
            .iter()
            .zip(&original_ids)
            .zip(["msg_", "fc_", "call_"])
        {
            assert_ne!(id, raw, "local IDs must never pass through as provider IDs");
            let suffix = id.as_str().unwrap().strip_prefix(prefix).unwrap();
            assert_eq!(uuid::Uuid::parse_str(suffix).unwrap().get_version_num(), 7);
        }
        assert_eq!(first["input"][2]["call_id"], first["input"][3]["call_id"]);
        let retry = project(&store, original.clone(), transport, NAMESPACE, KEY).await;
        assert_eq!(identifiers(&wire(&retry)), first_ids);
        drop(retry);
        for changed in [
            local_history("session-a", "synthetic task-b"),
            local_history("session-b", "synthetic task-a"),
        ] {
            let next = project(&store, changed, transport, NAMESPACE, KEY).await;
            let next = wire(&next);
            for (left, right) in first_ids.iter().zip(identifiers(&next)) {
                assert_ne!(
                    *left, right,
                    "content and session must each separate local IDs"
                );
            }
            assert_eq!(next["input"][2]["call_id"], next["input"][3]["call_id"]);
        }
        for (namespace, key) in [(NAMESPACE, "other-key"), ("other-account", KEY)] {
            let next = project(&store, original.clone(), transport, namespace, key).await;
            assert_ne!(identifiers(&wire(&next)), first_ids);
        }
    }
}

#[tokio::test]
async fn local_id_retry_survives_restart_and_generated_metadata_changes() {
    let (temp, store) = store();
    let mut body = local_history("stable-session", "synthetic stable content");
    let first = prepare(&store, body.clone()).await.unwrap();
    let original = identifiers(&wire(&first));
    drop(first);
    drop(store);
    let reopened = RequestStateStore::new(temp.path().to_path_buf());
    for item in body["input"].as_array_mut().unwrap() {
        item["internal_chat_message_metadata_passthrough"] = json!({"create_time":123.5});
    }
    body["input"][1]["status"] = "completed".into();
    body["input"][1]["content"][0]["annotations"] = json!([]);
    body["input"][1]["content"][0]["logprobs"] = json!([]);
    let replay = prepare(&reopened, body).await.unwrap();
    assert_eq!(identifiers(&wire(&replay)), original);
    let ledger = std::fs::read_to_string(reopened.state_path_for_test(NAMESPACE)).unwrap();
    assert!(!ledger.contains("synthetic stable content"));
}

#[tokio::test]
async fn original_ids_and_semantics_remain_significant_but_object_order_does_not() {
    let (_temp, store) = store();
    let body = local_history("stable-session", "synthetic task");
    let first = prepare(&store, body.clone()).await.unwrap();
    let original = identifiers(&wire(&first));
    drop(first);
    let reordered = serde_json::from_slice(&crate::subscription_index::canonical(&body)).unwrap();
    let repeated = prepare(&store, reordered).await.unwrap();
    assert_eq!(identifiers(&wire(&repeated)), original);
    drop(repeated);
    let mut changed = body.clone();
    changed["input"][1]["id"] = "msg_pi_3".into();
    changed["input"][2]["call_id"] = "call_pi_3".into();
    changed["input"][3]["call_id"] = "call_pi_3".into();
    changed["input"][3]["output"] = "opaque payload containing call_pi_1".into();
    let next = prepare(&store, changed).await.unwrap();
    let next = wire(&next);
    for (before, after) in original.iter().zip(identifiers(&next)) {
        assert_ne!(*before, after);
    }
    assert_eq!(next["input"][2]["call_id"], next["input"][3]["call_id"]);
    assert_eq!(
        next["input"][3]["output"],
        "opaque payload containing call_pi_1"
    );
}

#[tokio::test]
async fn anonymous_new_history_does_not_reuse_local_ids_or_legacy_raw_aliases() {
    let (_temp, store) = store();
    let legacy = store
        .edit(NAMESPACE, OWNER, KEY, |editor| {
            editor.wire_from_downstream(crate::request_state_types::WireIdDomain::Item, "msg_pi_1")
        })
        .await
        .unwrap();
    let mut body = local_history("unused", "synthetic task-a");
    body.as_object_mut().unwrap().remove("client_metadata");
    let first = prepare(&store, body.clone()).await.unwrap();
    let ids = identifiers(&wire(&first));
    let session = first.resolved_identity.as_ref().unwrap().session_id.clone();
    assert_ne!(ids[0], legacy);
    publish(&store, first, "resp_content_a", json!([])).await;
    let replay = prepare(&store, body.clone()).await.unwrap();
    assert_eq!(
        replay.resolved_identity.as_ref().unwrap().session_id,
        session
    );
    assert_eq!(identifiers(&wire(&replay)), ids);
    drop(replay);
    body["input"][1]["content"][0]["text"] = "synthetic task-b".into();
    let next = prepare(&store, body).await.unwrap();
    assert_ne!(next.resolved_identity.as_ref().unwrap().session_id, session);
    for (a, b) in ids.iter().zip(identifiers(&wire(&next))) {
        assert_ne!(*a, b);
    }
}
