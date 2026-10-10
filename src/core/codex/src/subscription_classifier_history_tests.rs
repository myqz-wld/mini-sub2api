use super::*;
use std::time::{Duration, Instant};

#[path = "subscription_classifier_reference_tests.rs"]
mod reference_tests;

const META: &str = "internal_chat_message_metadata_passthrough";

fn source(session: &str, thread: &str, turn: &str) -> Value {
    let mut body = json!({"model":"gpt-5.5","input":"synthetic source",
        "client_metadata":{"session_id":session,"thread_id":thread,"turn_id":turn}});
    if session != thread {
        body["client_metadata"]["parent_thread_id"] = session.into();
    }
    body
}

fn classifier(
    session: &str,
    lease: &str,
    turn: &str,
    source: &str,
    parent: &str,
    input: Value,
) -> Value {
    json!({"model":"gpt-5.6-luna","include":["reasoning.encrypted_content"],"input":input,
        "client_metadata":{"session_id":session,"thread_id":lease,"turn_id":turn,
            "parent_turn_id":parent,"x-openai-subagent":"guardian",
            "x-codex-turn-metadata":json!({"session_id":session,"thread_id":lease,
                "turn_id":turn,"parent_turn_id":parent,"guardian_classifier_source_thread_id":source,
                "thread_source":"guardian_classifier","turn_trigger":"guardian_classifier"}).to_string()}})
}

fn initial() -> Value {
    json!([
        {"type":"additional_tools","id":"at_setup","role":"developer","tools":[]},
        {"type":"message","id":"msg_base","role":"developer","content":[{"type":"input_text","text":"synthetic classifier"}]},
        {"type":"message","id":"msg_evidence","role":"user","content":[{"type":"input_text","text":"synthetic evidence"}]}
    ])
}

async fn scoped(
    store: &RequestStateStore,
    body: Value,
    transport: EmulationTransport,
    namespace: &str,
    key: &str,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription1620,
        transport,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        1024 * 1024,
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

fn output(turn: &str, metadata: bool) -> Vec<Value> {
    let mut output = vec![
        json!({"type":"reasoning","id":"rs_classification","summary":[],
        "encrypted_content":"synthetic-retained-cipher"}),
        json!({"type":"message","id":"msg_score","role":"assistant",
            "content":[{"type":"output_text","text":"low"}]}),
    ];
    if metadata {
        for item in &mut output {
            item[META] = json!({"turn_id":turn});
        }
    }
    output
}

#[tokio::test]
async fn classifier_retained_full_history_preserves_ids_ciphertext_and_completed_lease_ownership() {
    use EmulationTransport::{Http, WebSocket};
    for (transport, next_transport) in [
        (Http, Http),
        (WebSocket, WebSocket),
        (Http, WebSocket),
        (WebSocket, Http),
    ] {
        for rotate in [false, true] {
            for metadata in [false, true] {
                let (_temp, store) = store();
                let root = scoped(
                    &store,
                    source("session", "session", "source-turn"),
                    transport,
                    NAMESPACE,
                    KEY,
                )
                .await
                .unwrap();
                publish(&store, root, "resp_root", json!([])).await;
                let body = classifier(
                    "session",
                    "lease-a",
                    "classification-a",
                    "session",
                    "source-turn",
                    initial(),
                );
                let first = scoped(&store, body, transport, NAMESPACE, KEY)
                    .await
                    .unwrap();
                let first_wire: Value = serde_json::from_slice(&first.body).unwrap();
                assert_eq!(
                    first_wire["include"],
                    json!(["reasoning.encrypted_content"])
                );
                assert_eq!(first_wire["input"][0]["tools"], json!([]));
                let old = first.resolved_identity.clone().unwrap();
                let response = publish(
                    &store,
                    first,
                    "resp_classification",
                    json!(output(old.turn_id.as_deref().unwrap(), metadata)),
                )
                .await;
                let mut history = initial();
                history
                    .as_array_mut()
                    .unwrap()
                    .extend(response["output"].as_array().unwrap().clone());
                // Native protocol models serialize an absent optional reasoning content as null.
                history[3]["content"] = Value::Null;
                history
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"role":"user","content":"new evidence"}));
                let next = classifier(
                    "session",
                    if rotate { "lease-b" } else { "lease-a" },
                    "classification-b",
                    "session",
                    "source-turn",
                    history,
                );
                let prepared = scoped(&store, next, next_transport, NAMESPACE, KEY)
                    .await
                    .unwrap();
                let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
                assert!(wire.get("previous_response_id").is_none());
                assert_eq!(wire["input"].as_array().unwrap().len(), 6);
                assert_eq!(wire["input"][0]["id"], first_wire["input"][0]["id"]);
                assert_eq!(
                    wire["input"][3]["encrypted_content"],
                    "synthetic-retained-cipher"
                );
                assert_eq!(wire["input"][3]["id"], "rs_classification");
                if metadata {
                    assert_eq!(
                        wire["input"][4][META]["turn_id"],
                        old.turn_id.clone().unwrap()
                    );
                }
                let next_id = prepared.resolved_identity.as_ref().unwrap();
                assert_eq!(old.thread_id != next_id.thread_id, rotate);
                assert_ne!(old.turn_id, next_id.turn_id);
            }
        }
    }
}

#[tokio::test]
async fn classifier_history_rejects_missing_completion_scope_forgery_and_expired_evidence() {
    for mode in [
        "early",
        "eof",
        "conflicting-completion",
        "failed",
        "incomplete",
        "expired",
        "restart",
        "key",
        "account",
        "session",
        "source",
        "ordinary",
        "changed-body",
        "changed-id",
        "appended-old-turn",
        "unknown-turn",
    ] {
        let (temp, store) = store();
        let transport = EmulationTransport::Http;
        let root = scoped(
            &store,
            source("session", "session", "source-turn"),
            transport,
            NAMESPACE,
            KEY,
        )
        .await
        .unwrap();
        publish(&store, root, "resp_root", json!([])).await;
        let first = scoped(
            &store,
            classifier(
                "session",
                "lease-a",
                "classification-a",
                "session",
                "source-turn",
                initial(),
            ),
            transport,
            NAMESPACE,
            KEY,
        )
        .await
        .unwrap();
        let identity = first.resolved_identity.clone().unwrap();
        let context =
            ResponseStateContext::new(OWNER, NAMESPACE, KEY, &store, Some(&identity), None)
                .with_operation(first.operation);
        context
            .translate_value(
                json!({"type":"response.created","response":{"id":"resp_classification"}}),
            )
            .await
            .unwrap();
        context.translate_value(json!({"type":"response.output_text.delta","item_id":"msg_score","output_index":1,"content_index":0,"delta":"low"})).await.unwrap();
        assert!(
            store.contexts.inner.lock().unwrap().scopes[&ContextStore::scope_key(NAMESPACE, KEY)]
                .records
                .values()
                .all(|r| r.identity.request_kind != "guardian_classifier" || !r.completed)
        );
        let mut retained = Vec::new();
        let items = output(identity.turn_id.as_deref().unwrap(), true);
        for (index, item) in items.iter().enumerate() {
            let event = context
                .translate_value(
                    json!({"type":"response.output_item.done","output_index":index,"item":item}),
                )
                .await
                .unwrap();
            retained.push(event["item"].clone());
        }
        if !matches!(mode, "early" | "eof") {
            let kind = match mode {
                "failed" => "response.failed",
                "incomplete" => "response.incomplete",
                _ => "response.completed",
            };
            let mut terminal_items = items.clone();
            if mode == "conflicting-completion" {
                terminal_items[0]["encrypted_content"] = "conflicting-terminal-cipher".into();
            }
            let result = context
                .translate_value(
                    json!({"type":kind,"response":{"id":"resp_classification","output":terminal_items,
                "incomplete_details":{"reason":"content_filter"}}}),
                )
                .await;
            if mode == "conflicting-completion" {
                assert!(
                    result.is_err(),
                    "conflicting classifier footer became completion"
                );
            } else {
                result.unwrap();
            }
        }
        if mode == "eof" {
            drop(context);
        }
        let (namespace, key, session) = (
            if mode == "account" {
                "other-account"
            } else {
                NAMESPACE
            },
            if mode == "key" { "other-key" } else { KEY },
            if mode == "session" {
                "other-session"
            } else {
                "session"
            },
        );
        if matches!(mode, "key" | "account" | "session") {
            let root = scoped(
                &store,
                source(
                    session,
                    session,
                    if mode == "session" {
                        "other-source-turn"
                    } else {
                        "source-turn"
                    },
                ),
                transport,
                namespace,
                key,
            )
            .await
            .unwrap();
            drop(root);
        }
        let (source_thread, parent) = if mode == "source" {
            let sibling = scoped(
                &store,
                source("session", "sibling", "sibling-turn"),
                transport,
                NAMESPACE,
                KEY,
            )
            .await
            .unwrap();
            drop(sibling);
            ("sibling", "sibling-turn")
        } else {
            (
                session,
                if mode == "session" {
                    "other-source-turn"
                } else {
                    "source-turn"
                },
            )
        };
        let mut history = initial();
        history.as_array_mut().unwrap().extend(retained.clone());
        history
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":"next evidence"}));
        if mode == "changed-body" {
            history[1]["content"][0]["text"] = "forged prefix".into();
        }
        if mode == "changed-id" {
            history[4]["id"] = "msg_forged".into();
        }
        if mode == "unknown-turn" {
            history[4][META]["turn_id"] = "unknown-classifier-turn".into();
        }
        if mode == "appended-old-turn" {
            history.as_array_mut().unwrap().push(retained[1].clone());
        }
        let mut next = classifier(
            session,
            "lease-b",
            "classification-b",
            source_thread,
            parent,
            history,
        );
        if mode == "ordinary" {
            next["model"] = "gpt-5.5".into();
            next["client_metadata"] = json!({"session_id":"session","thread_id":"ordinary-sibling","parent_thread_id":"session","turn_id":"ordinary-next"});
        }
        if mode == "expired" {
            store
                .contexts
                .inner
                .lock()
                .unwrap()
                .expire(Instant::now() + Duration::from_secs(4 * 60 * 60));
        }
        let reopened;
        let active = if mode == "restart" {
            reopened = RequestStateStore::new(temp.path().to_path_buf());
            &reopened
        } else {
            &store
        };
        let result = scoped(active, next, transport, namespace, key).await;
        assert!(
            result.is_err(),
            "unproved classifier history admitted: {mode}"
        );
    }
}

#[tokio::test]
async fn classifier_snapshot_keeps_empty_include_unless_ciphertext_was_explicitly_requested() {
    for include in [
        None,
        Some(Value::Null),
        Some(json!([])),
        Some(json!(["reasoning.encrypted_content"])),
    ] {
        let (_temp, store) = store();
        let root = prepare(&store, source("session", "session", "source-turn"))
            .await
            .unwrap();
        drop(root);
        let mut body = classifier(
            "session",
            "lease",
            "classification",
            "session",
            "source-turn",
            initial(),
        );
        body.as_object_mut().unwrap().remove("include");
        if let Some(include) = &include {
            body["include"] = include.clone();
        }
        let prepared = prepare(&store, body).await.unwrap();
        let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert_eq!(
            wire["include"],
            if include.as_ref().is_some_and(Value::is_array) {
                include.unwrap()
            } else {
                json!([])
            }
        );
        assert_eq!(wire["input"][0]["type"], "additional_tools");
        assert_eq!(wire["input"][0]["tools"], json!([]));
    }
}
