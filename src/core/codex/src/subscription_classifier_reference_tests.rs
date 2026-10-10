use super::*;
use crate::request_identity_projection::ResolvedRequestIdentity;
use crate::subscription_context::SocketLease;

async fn seed_reference(
    store: &RequestStateStore,
    http: bool,
    effort: Option<&Value>,
) -> (SocketLease, ResolvedRequestIdentity, Value, Value) {
    let root = prepare(store, source("session", "session", "source-turn"))
        .await
        .unwrap();
    publish(store, root, "resp_reference_root", json!([])).await;
    let socket = store.contexts.open_socket().unwrap();
    let mut first = classifier(
        "session",
        "lease-a",
        "classification-a",
        "session",
        "source-turn",
        initial(),
    );
    if let Some(effort) = effort {
        first["reasoning"] = json!({"effort":effort});
    }
    let first = if http {
        prepare(store, first).await.unwrap()
    } else {
        first["type"] = "response.create".into();
        repair_tests::websocket_request(store, first, &socket.id, None)
            .await
            .unwrap()
    };
    let identity = first.resolved_identity.clone().unwrap();
    let wire = serde_json::from_slice(&first.body).unwrap();
    let response = publish(
        store,
        first,
        "resp_reference_classification",
        json!(output(identity.turn_id.as_deref().unwrap(), true)),
    )
    .await;
    (socket, identity, response, wire)
}

fn continuation(response: &Value, effort: Option<&Value>) -> Value {
    let mut next = classifier(
        "session",
        "lease-a",
        "classification-b",
        "session",
        "source-turn",
        json!([{"role":"user","content":"new evidence"}]),
    );
    next["previous_response_id"] = response["id"].clone();
    if let Some(effort) = effort {
        next["reasoning"] = json!({"effort":effort});
    }
    next
}

#[tokio::test]
async fn classifier_references_rebuild_complete_input_and_keep_role_effort_on_every_transport() {
    for mode in ["http", "same-socket", "reconnect"] {
        for effort in [
            None,
            Some(json!("high")),
            Some(json!(8192)),
            Some(json!("8192")),
        ] {
            let (_temp, store) = store();
            let (socket, identity, response, first_wire) =
                seed_reference(&store, mode == "http", effort.as_ref()).await;
            let mut next = continuation(&response, effort.as_ref());
            let next = if mode == "http" {
                prepare(&store, next).await.unwrap()
            } else {
                next["type"] = "response.create".into();
                let fresh_socket = store.contexts.open_socket().unwrap();
                let (id, binding) = if mode == "same-socket" {
                    (&socket.id, Some(&identity))
                } else {
                    (&fresh_socket.id, None)
                };
                repair_tests::websocket_request(&store, next, id, binding)
                    .await
                    .unwrap()
            };
            let wire: Value = serde_json::from_slice(&next.body).unwrap();
            assert!(next.rebuilt_reference, "{mode}");
            assert!(wire.get("previous_response_id").is_none(), "{mode}");
            let items = wire["input"].as_array().unwrap();
            assert_eq!(items.len(), 6, "complete classifier source missing: {mode}");
            assert_eq!(&items[..3], first_wire["input"].as_array().unwrap());
            assert_eq!(items[3]["encrypted_content"], "synthetic-retained-cipher");
            assert_eq!(items[4]["role"], "assistant");
            assert_eq!(items[5]["content"][0]["text"], "new evidence");
            let expected = match effort.as_ref() {
                None => json!("low"),
                Some(Value::String(value)) if value == "8192" => json!(8192),
                Some(value) => value.clone(),
            };
            assert_eq!(first_wire["reasoning"]["effort"], expected);
            assert_eq!(wire["reasoning"]["effort"], expected, "{mode}");
            assert_eq!(wire["include"], json!(["reasoning.encrypted_content"]));
            assert_eq!(wire["tool_choice"], "none");
            assert!(wire.get("text").is_none());
        }
    }
}

#[tokio::test]
async fn classifier_reference_requires_local_history_even_while_the_original_socket_is_live() {
    for expired in [false, true] {
        let (_temp, store) = store();
        let (socket, identity, response, _) = seed_reference(&store, false, None).await;
        let scope = ContextStore::scope_key(NAMESPACE, KEY);
        {
            let mut inner = store.contexts.inner.lock().unwrap();
            if expired {
                inner.expire(Instant::now() + Duration::from_secs(4 * 60 * 60));
            } else {
                inner
                    .scopes
                    .get_mut(&scope)
                    .unwrap()
                    .records
                    .get_mut(response["id"].as_str().unwrap())
                    .unwrap()
                    .history = None;
            }
            let record = &inner.scopes[&scope].records[response["id"].as_str().unwrap()];
            assert!(record.completed && record.history.is_none());
            assert_eq!(record.socket.as_deref(), Some(socket.id.as_str()));
            assert!(inner.sockets.contains(&socket.id));
        }
        let ledger = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
        let mut next = continuation(&response, None);
        next["type"] = "response.create".into();
        let result =
            repair_tests::websocket_request(&store, next, &socket.id, Some(&identity)).await;
        assert!(matches!(
            result,
            Err(StatefulPrepareError::StateUnavailable)
        ));
        assert_eq!(
            std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap(),
            ledger
        );
        let inner = store.contexts.inner.lock().unwrap();
        assert!(inner.operations.is_empty() && inner.reservations.is_empty());
    }
}

#[test]
fn history_settings_use_header_roles_and_the_classifier_model_before_defaulting() {
    use crate::subscription_index::settings_for_format;
    use crate::subscription_request::{Evidence, Format};
    for marker in ["classifier", "reviewer"] {
        for effort in [None, Some(json!("high"))] {
            let mut headers = HeaderMap::new();
            headers.insert("x-codex-guardian", marker.parse().unwrap());
            let mut body = json!({"model":"gpt-6-sol","input":"synthetic evidence"});
            if let Some(effort) = &effort {
                body["reasoning"] = json!({"effort":effort});
            }
            let evidence = Evidence::read(
                body.as_object().unwrap(),
                &headers,
                EmulationTransport::Http,
            )
            .unwrap();
            let settings = settings_for_format(
                body.as_object().unwrap(),
                Format::Responses,
                evidence.transport,
                evidence.role,
            );
            assert_eq!(
                settings["reasoning"]["effort"],
                effort.unwrap_or(json!("low"))
            );
            if marker == "classifier" {
                assert_eq!(settings["model"], "gpt-5.6-luna");
                assert_eq!(settings["reasoning"]["context"], "all_turns");
                assert_eq!(settings["include"], json!([]));
                assert_eq!(settings["tool_choice"], "none");
                assert!(settings.get("text").is_none());
            }
        }
    }
}
