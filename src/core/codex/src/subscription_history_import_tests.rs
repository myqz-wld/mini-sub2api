use super::*;
use std::time::{Duration, Instant};

fn historical(turn: &str) -> Value {
    json!({"type":"message","id":"msg_history_copy","role":"assistant",
        "content":[{"type":"output_text","text":"Synthetic historical answer"}],
        "internal_chat_message_metadata_passthrough":{"turn_id":turn}})
}

async fn seed(store: &RequestStateStore) -> (String, String, String) {
    let mut first = request(json!([input("Synthetic source")]));
    first["client_metadata"] = json!({"session_id":"source-session","turn_id":"source-turn"});
    let prepared = prepare(store, first).await.unwrap();
    let source = prepared.resolved_identity.as_ref().unwrap().clone();
    let response = publish(
        store,
        prepared,
        "resp_history_source",
        json!([historical(source.turn_id.as_deref().unwrap())]),
    )
    .await;
    (
        response["output"][0]["internal_chat_message_metadata_passthrough"]["turn_id"]
            .as_str()
            .unwrap()
            .into(),
        source.thread_id,
        response["id"].as_str().unwrap().into(),
    )
}

fn expire(store: &RequestStateStore) {
    store
        .contexts
        .inner
        .lock()
        .unwrap()
        .expire(Instant::now() + Duration::from_secs(4 * 60 * 60));
}

async fn prepare_transport(
    store: &RequestStateStore,
    body: Value,
    transport: EmulationTransport,
    key: &str,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    prepare_scope(store, body, transport, key, NAMESPACE).await
}

async fn prepare_scope(
    store: &RequestStateStore,
    body: Value,
    transport: EmulationTransport,
    key: &str,
    namespace: &str,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription1560,
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
}

#[tokio::test]
async fn import_is_stable_for_http_ws_and_ordinary_lite_across_restart() {
    for transport in [EmulationTransport::Http, EmulationTransport::WebSocket] {
        for model in ["gpt-5.4", "gpt-6-astra"] {
            let (temp, store) = store();
            let (turn, source_thread, _) = seed(&store).await;
            expire(&store);
            let mut body = request(json!([
                input("Synthetic source"),
                historical(&turn),
                input("Continue")
            ]));
            body["model"] = model.into();
            if model == "gpt-6-astra" {
                // Empty optional IDs are absent identities, not conflicting declarations.
                body["input"][0]["id"] = "".into();
                body["input"][1]["id"] = "".into();
            }
            let first = prepare_transport(&store, body.clone(), transport, KEY)
                .await
                .unwrap();
            let target = first.resolved_identity.as_ref().unwrap().clone();
            assert_ne!(target.thread_id, source_thread);
            let wire: Value = serde_json::from_slice(&first.body).unwrap();
            let imported = wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["role"] == "assistant")
                .unwrap()["internal_chat_message_metadata_passthrough"]["turn_id"]
                .clone();
            publish(&store, first, "resp_imported_first", json!([])).await;
            body["input"]
                .as_array_mut()
                .unwrap()
                .push(input("Another turn"));
            let second = prepare_transport(&store, body.clone(), transport, KEY)
                .await
                .unwrap();
            assert_eq!(
                second.resolved_identity.as_ref().unwrap().thread_id,
                target.thread_id
            );
            drop(second);
            // An explicit current session can recover its imported copies even when all bodies vanish.
            let reopened = RequestStateStore::new(temp.path().to_path_buf());
            body["client_metadata"] = json!({"session_id":target.session_id});
            let third = prepare_transport(&reopened, body, transport, KEY)
                .await
                .unwrap();
            let wire: Value = serde_json::from_slice(&third.body).unwrap();
            let item = wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["role"] == "assistant")
                .unwrap();
            assert_eq!(
                item["internal_chat_message_metadata_passthrough"]["turn_id"],
                imported
            );
        }
    }
}

#[tokio::test]
async fn import_rejects_retained_sources_references_and_incomplete_dependencies() {
    for case in [
        "retained",
        "previous",
        "explicit-target",
        "external-conversation",
        "item-reference",
        "missing-call",
        "open-call",
        "duplicate-result",
        "missing-cipher",
        "assistant-only",
    ] {
        let (_temp, store) = store();
        let (turn, _, previous) = seed(&store).await;
        if case != "retained" {
            expire(&store);
        }
        let mut body = request(json!([
            input("Synthetic source"),
            historical(&turn),
            input("Continue")
        ]));
        match case {
            "previous" => body["previous_response_id"] = previous.into(),
            "explicit-target" => {
                body["client_metadata"] = json!({"session_id":"unrelated-session"})
            }
            "external-conversation" => body["conversation"] = json!({"id":"remote-conversation"}),
            "item-reference" => body["input"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"item_reference","id":"msg_history_copy"})),
            "missing-call" => body["input"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"function_call_output","call_id":"missing","output":"value"})),
            "open-call" | "duplicate-result" => {
                body["input"].as_array_mut().unwrap().push(json!({"type":"function_call","call_id":"call","name":"inspect","arguments":"{}"}));
                if case == "duplicate-result" {
                    for _ in 0..2 {
                        body["input"].as_array_mut().unwrap().push(json!({"type":"function_call_output","call_id":"call","output":"value"}));
                    }
                }
            }
            "missing-cipher" => body["input"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"reasoning","summary":[]})),
            "assistant-only" => body["input"] = json!([historical(&turn)]),
            _ => {}
        }
        let before = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
        assert!(
            prepare(&store, body).await.is_err(),
            "import unexpectedly accepted {case}"
        );
        assert_eq!(
            std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
            before,
            "rejected import changed durable state: {case}"
        );
        assert!(store.contexts.inner.lock().unwrap().reservations.is_empty());
    }
}

#[tokio::test]
async fn import_never_adopts_a_source_that_is_still_running() {
    let (_temp, store) = store();
    let mut source = request(json!([input("Running source")]));
    source["client_metadata"] = json!({"session_id":"busy-source","turn_id":"busy-turn"});
    let active = prepare(&store, source).await.unwrap();
    let turn = active
        .resolved_identity
        .as_ref()
        .unwrap()
        .turn_id
        .as_deref()
        .unwrap();
    let body = request(json!([
        input("Running source"),
        historical(turn),
        input("Independent input")
    ]));
    let before = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
    assert!(prepare(&store, body).await.is_err());
    assert_eq!(
        std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
        before
    );
    assert_eq!(store.contexts.inner.lock().unwrap().operations.len(), 1);
}

#[tokio::test]
async fn imported_history_follows_target_ancestry_without_authorizing_unrelated_threads() {
    let (_temp, store) = store();
    let (turn, _, _) = seed(&store).await;
    expire(&store);
    let history = json!([
        input("Synthetic source"),
        historical(&turn),
        input("Import history")
    ]);
    let imported = prepare(&store, request(history.clone())).await.unwrap();
    let target = imported.resolved_identity.as_ref().unwrap().clone();
    let projected: Value = serde_json::from_slice(&imported.body).unwrap();
    let copy =
        projected["input"][1]["internal_chat_message_metadata_passthrough"]["turn_id"].clone();
    publish(&store, imported, "resp_target_import", json!([])).await;
    for mode in ["child", "fork", "unrelated"] {
        let mut body = request(history.clone());
        body["input"]
            .as_array_mut()
            .unwrap()
            .push(input("Child task"));
        body["client_metadata"] = match mode {
            "child" => {
                json!({"session_id":target.session_id,"thread_id":"import-child","parent_thread_id":target.thread_id,"turn_id":"child-turn"})
            }
            "fork" => json!({"session_id":"import-fork","turn_id":"fork-turn",
                "x-codex-turn-metadata":json!({"forked_from_thread_id":target.thread_id}).to_string()}),
            _ => json!({"session_id":"unrelated-session","turn_id":"unrelated-turn"}),
        };
        let result = prepare(&store, body).await;
        if mode != "unrelated" {
            let child = result.unwrap();
            let wire: Value = serde_json::from_slice(&child.body).unwrap();
            assert_eq!(
                wire["input"][1]["internal_chat_message_metadata_passthrough"]["turn_id"],
                copy
            );
        } else {
            assert!(matches!(result, Err(StatefulPrepareError::InvalidRequest)));
        }
    }
}

#[tokio::test]
async fn import_uses_only_supplied_content_and_never_crosses_key_aliases() {
    let (_temp, store) = store();
    let mut source = request(json!([input("Synthetic source")]));
    source["include"] = json!([]);
    let first = prepare(&store, source).await.unwrap();
    let identity = first.resolved_identity.as_ref().unwrap().clone();
    let response = publish(&store, first, "resp_private_source", json!([
        {"type":"reasoning","id":"rs_private","summary":[],"encrypted_content":"synthetic-hidden-cipher"},
        historical(identity.turn_id.as_deref().unwrap())])).await;
    expire(&store);
    let body = request(json!([
        input("Synthetic source"),
        response["output"][1],
        input("Continue")
    ]));
    let imported = prepare(&store, body.clone()).await.unwrap();
    let public_turn =
        response["output"][1]["internal_chat_message_metadata_passthrough"]["turn_id"]
            .as_str()
            .unwrap()
            .to_owned();
    let other_account = prepare_scope(
        &store,
        body.clone(),
        EmulationTransport::Http,
        KEY,
        "other-account-namespace",
    )
    .await
    .unwrap();
    assert!(
        !std::str::from_utf8(&other_account.body)
            .unwrap()
            .contains("synthetic-hidden-cipher")
    );
    assert_ne!(
        other_account.resolved_identity.as_ref().unwrap().session_id,
        identity.session_id
    );
    let other = prepare_transport(&store, body, EmulationTransport::Http, "other-key")
        .await
        .unwrap();
    for prepared in [&imported, &other] {
        assert!(
            !std::str::from_utf8(&prepared.body)
                .unwrap()
                .contains("synthetic-hidden-cipher")
        );
        assert_ne!(
            prepared.resolved_identity.as_ref().unwrap().session_id,
            identity.session_id
        );
    }
    let own: Value = serde_json::from_slice(&imported.body).unwrap();
    let foreign: Value = serde_json::from_slice(&other.body).unwrap();
    assert_ne!(
        own["input"][1]["internal_chat_message_metadata_passthrough"]["turn_id"],
        foreign["input"][1]["internal_chat_message_metadata_passthrough"]["turn_id"]
    );
    store
        .edit(NAMESPACE, OWNER, "other-key", move |editor| {
            let alias = editor
                .existing_wire_from_downstream(
                    crate::request_state_types::WireIdDomain::Turn,
                    &public_turn,
                )?
                .unwrap();
            assert!(
                editor.turn_by_id(&alias).is_none(),
                "another Key adopted private turn ownership"
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn mixed_source_rejection_rolls_back_already_prepared_imports() {
    let (_temp, store) = store();
    let (expired_turn, _, _) = seed(&store).await;
    expire(&store);
    let mut second = request(json!([input("Retained source")]));
    second["client_metadata"] = json!({"session_id":"retained-session","turn_id":"retained-turn"});
    let prepared = prepare(&store, second).await.unwrap();
    let retained_turn = prepared
        .resolved_identity
        .as_ref()
        .unwrap()
        .turn_id
        .clone()
        .unwrap();
    publish(&store, prepared, "resp_retained", json!([])).await;
    let before = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
    for duplicate_id in [false, true] {
        let mut second = historical(&retained_turn);
        if !duplicate_id {
            second["id"] = "msg_retained_copy".into();
        }
        let body = request(json!([
            input("Synthetic source"),
            historical(&expired_turn),
            input("Retained source"),
            second,
            input("Combine supplied content")
        ]));
        assert!(matches!(
            prepare(&store, body).await,
            Err(StatefulPrepareError::InvalidRequest)
        ));
        assert_eq!(
            std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
            before
        );
        assert!(store.contexts.inner.lock().unwrap().reservations.is_empty());
    }
}

#[tokio::test]
async fn existing_import_cannot_mask_an_unrelated_turn_on_a_repeated_item_id() {
    let (_temp, store) = store();
    let (old, _, _) = seed(&store).await;
    expire(&store);
    let first = request(json!([
        input("Synthetic source"),
        historical(&old),
        historical(&old),
        input("Continue")
    ]));
    let prepared = prepare(&store, first.clone()).await.unwrap();
    let target = prepared
        .resolved_identity
        .as_ref()
        .unwrap()
        .session_id
        .clone();
    publish(&store, prepared, "resp_repeat_import", json!([])).await;
    let mut other = request(json!([input("Other source")]));
    other["client_metadata"] = json!({"session_id":"other-session","turn_id":"other-turn"});
    let prepared = prepare(&store, other).await.unwrap();
    let other = prepared
        .resolved_identity
        .as_ref()
        .unwrap()
        .turn_id
        .clone()
        .unwrap();
    publish(&store, prepared, "resp_other_repeat", json!([])).await;
    let before = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
    let mut conflict = first;
    conflict["client_metadata"] = json!({"session_id":target});
    conflict["input"]
        .as_array_mut()
        .unwrap()
        .push(historical(&other));
    assert!(matches!(
        prepare(&store, conflict).await,
        Err(StatefulPrepareError::InvalidRequest)
    ));
    assert_eq!(
        std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
        before
    );
}
