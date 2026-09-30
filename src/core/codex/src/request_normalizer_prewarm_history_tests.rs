use super::*;
use crate::request_state_types::{WireIdDomain, WireIdOwner};
use serde_json::json;

const ACCOUNT: &str = "acct_prewarm_history";
const NAMESPACE: &str = "synthetic-prewarm-history";
const SCOPE: &str = "synthetic-prewarm-key";
const META: &str = "internal_chat_message_metadata_passthrough";

fn request(model: &str, session: &str, turn: &str, input: Value, prewarm: bool) -> Value {
    let mut body = json!({"type":"response.create","model":model,"input":input,
        "instructions":"Synthetic caller base", "tools":[],
        "client_metadata":{"session_id":session,"thread_id":session,"turn_id":turn,
            "x-codex-turn-metadata":json!({"session_id":session,"thread_id":session,
                "turn_id":turn,"request_kind":if prewarm {"prewarm"} else {"turn"}}).to_string()}});
    if prewarm {
        body["generate"] = false.into();
    }
    body
}

fn message(role: &str, turn: &str) -> Value {
    json!({"type":"message","role":role,"content":[{
        "type":if role == "assistant" {"output_text"} else {"input_text"},
        "text":"Synthetic history"}], META:{"turn_id":turn}})
}

async fn prepare(
    store: &RequestStateStore,
    scope: &str,
    body: Value,
) -> Result<PreparedEmulatedRequest, StatefulPrepareError> {
    prepare_identity_request(
        UpstreamProfile::CodexSubscription1592,
        EmulationTransport::WebSocket,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
        1024 * 1024,
        CodexStateContext {
            account_ref: ACCOUNT,
            state_namespace: NAMESPACE,
            downstream_scope: scope,
            fingerprint_mode: FingerprintMode::Device,
            store,
            binding: None,
            force_lite: false,
            admission: None,
            socket_id: None,
        },
        false,
    )
    .await
}

fn value(prepared: &PreparedEmulatedRequest) -> Value {
    serde_json::from_slice(&prepared.body).unwrap()
}

fn assert_empty_turn(prepared: &PreparedEmulatedRequest) {
    let identity = prepared.resolved_identity.as_ref().unwrap();
    assert_eq!(identity.turn_id.as_deref(), Some(""));
    assert!(identity.root_turn_id.is_none());
    assert!(identity.parent_turn_id.is_none());
    assert!(identity.turn_started_at_unix_ms.is_none());
    let body = value(prepared);
    assert_eq!(body["generate"], false);
    assert_eq!(body["client_metadata"]["turn_id"], "");
    let nested: Value = serde_json::from_str(
        body["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(nested["turn_id"], "");
    for name in ["root_turn_id", "parent_turn_id", "turn_started_at_unix_ms"] {
        assert!(nested.get(name).is_none());
        assert!(body["client_metadata"].get(name).is_none());
    }
}

#[tokio::test]
async fn history_prewarm_preserves_known_turns_without_changing_active_turn() {
    for model in ["gpt-5.5", "gpt-6-sol"] {
        let harness = CodexStateTestHarness::new();
        let mut known = Vec::new();
        for raw in ["history-first", "history-second"] {
            let prepared = prepare(
                &harness.store,
                SCOPE,
                request(
                    model,
                    "history-session",
                    raw,
                    json!([message("user", raw)]),
                    false,
                ),
            )
            .await
            .unwrap();
            known.push(prepared.resolved_identity.unwrap());
        }
        // End with the older turn to catch accidental current-turn adoption.
        let input = json!([
            message("user", "history-second"),
            message("assistant", "history-second"),
            message("user", "history-first")
        ]);
        let warm = prepare(
            &harness.store,
            SCOPE,
            request(model, "history-session", "", input, true),
        )
        .await
        .unwrap();
        assert_empty_turn(&warm);
        let wire = value(&warm);
        let items = wire["input"].as_array().unwrap();
        let prefix = items.len() - 3;
        for item in &items[..prefix] {
            assert!(
                item.get(META).is_none(),
                "generated native setup stays unstamped"
            );
        }
        for (item, expected) in items[prefix..]
            .iter()
            .zip([&known[1], &known[1], &known[0]])
        {
            assert_eq!(item[META]["turn_id"].as_str(), expected.turn_id.as_deref());
        }
        harness
            .store
            .edit(NAMESPACE, ACCOUNT, SCOPE, move |editor| {
                assert_eq!(
                    editor.current_turn_id(&known[1].thread_id),
                    known[1].turn_id
                );
                for identity in &known {
                    let (_, turn) = editor
                        .turn_by_id(identity.turn_id.as_deref().unwrap())
                        .unwrap();
                    assert_eq!(turn.thread_id, identity.thread_id);
                }
                Ok(())
            })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn history_prewarm_unknown_turns_remain_unowned_stable_and_adoptable_after_reload() {
    for model in ["gpt-5.5", "gpt-6-sol"] {
        let harness = CodexStateTestHarness::new();
        let mut history = json!([message("user", "import-first"),
            {"type":"reasoning","summary":[],"encrypted_content":"synthetic-ciphertext",
                META:{"turn_id":"import-first"}},
            {"type":"custom_tool_call","call_id":"call_import","name":"exec","input":"synthetic()",
                META:{"turn_id":"import-first"}},
            {"type":"custom_tool_call_output","call_id":"call_import","output":"synthetic result",
                META:{"turn_id":"import-first","cell_id":"call_import",
                    "executed_tool_calls":[{"name":"tools.probe","arguments":{}}],"tool_calls_complete":true}},
            message("assistant", "import-first"), message("user", "import-second")]);
        let body = request(model, "import-session", "", history.clone(), true);
        let warm = prepare(&harness.store, SCOPE, body.clone()).await.unwrap();
        assert_empty_turn(&warm);
        let first = value(&warm);
        let items = first["input"].as_array().unwrap();
        let offset = items.len() - 6;
        let first_turn = items[offset][META]["turn_id"].as_str().unwrap();
        let second_turn = items[offset + 5][META]["turn_id"].as_str().unwrap();
        assert_ne!(first_turn, "import-first");
        assert_ne!(second_turn, "import-second");
        assert_ne!(first_turn, second_turn);
        assert!(!first_turn.is_empty() && !second_turn.is_empty());
        for item in &items[offset..offset + 5] {
            assert_eq!(item[META]["turn_id"], first_turn);
        }
        assert_eq!(
            items[offset + 1]["encrypted_content"],
            "synthetic-ciphertext"
        );
        assert_eq!(
            items[offset + 3][META]["cell_id"],
            items[offset + 2]["call_id"]
        );
        assert_ne!(items[offset + 2]["call_id"], "call_import");
        let unowned_turns = [first_turn.to_owned(), second_turn.to_owned()];
        let thread = warm.resolved_identity.as_ref().unwrap().thread_id.clone();
        harness
            .store
            .edit(NAMESPACE, ACCOUNT, SCOPE, move |editor| {
                for turn in unowned_turns {
                    assert!(editor.turn_by_id(&turn).is_none());
                }
                assert!(editor.current_turn_id(&thread).is_none());
                Ok(())
            })
            .await
            .unwrap();
        let reopened = RequestStateStore::new(harness._temp.path().join("accounts"));
        let again = prepare(&reopened, SCOPE, body).await.unwrap();
        assert_eq!(
            value(&again)["input"],
            first["input"],
            "history projection survives reload"
        );
        history
            .as_array_mut()
            .unwrap()
            .push(message("user", "import-second"));
        let sampling = prepare(
            &reopened,
            SCOPE,
            request(model, "import-session", "import-second", history, false),
        )
        .await
        .unwrap();
        assert_eq!(
            sampling.resolved_identity.unwrap().turn_id.as_deref(),
            Some(second_turn)
        );
    }
}

#[tokio::test]
async fn history_prewarm_enforces_ancestor_and_declared_fork_eligibility() {
    for model in ["gpt-5.5", "gpt-6-sol"] {
        let harness = CodexStateTestHarness::new();
        let source = prepare(
            &harness.store,
            SCOPE,
            request(
                model,
                "source",
                "source-turn",
                json!([message("user", "source-turn")]),
                false,
            ),
        )
        .await
        .unwrap()
        .resolved_identity
        .unwrap();
        for relationship in ["same", "child", "fork", "unrelated"] {
            let session = if relationship == "same" || relationship == "child" {
                "source"
            } else {
                relationship
            };
            let mut body = request(
                model,
                session,
                "",
                json!([message("user", "source-turn")]),
                true,
            );
            if relationship == "child" {
                body["client_metadata"]["thread_id"] = "child".into();
                body["client_metadata"]["parent_thread_id"] = "source".into();
                body["client_metadata"]["x-codex-turn-metadata"] = json!({
                    "request_kind":"prewarm","turn_id":"","session_id":"source",
                    "thread_id":"child","parent_thread_id":"source"})
                .to_string()
                .into();
            } else if relationship == "fork" {
                body["client_metadata"]["forked_from_thread_id"] = "source".into();
            }
            let result = prepare(&harness.store, SCOPE, body).await;
            if relationship == "unrelated" {
                assert!(matches!(result, Err(StatefulPrepareError::InvalidRequest)));
            } else {
                let warm = result.unwrap();
                assert_empty_turn(&warm);
                let body = value(&warm);
                assert_eq!(
                    body["input"].as_array().unwrap().last().unwrap()[META]["turn_id"].as_str(),
                    source.turn_id.as_deref()
                );
            }
        }
    }
}

#[tokio::test]
async fn history_prewarm_keeps_key_aliases_separate_and_rejects_foreign_response_reference() {
    let harness = CodexStateTestHarness::new();
    let body = request(
        "gpt-5.5",
        "shared-session",
        "",
        json!([message("user", "shared-turn")]),
        true,
    );
    let first = prepare(&harness.store, SCOPE, body.clone()).await.unwrap();
    let second = prepare(&harness.store, "other-key", body.clone())
        .await
        .unwrap();
    assert_ne!(
        value(&first)["input"][0][META]["turn_id"],
        value(&second)["input"][0][META]["turn_id"]
    );
    let identity = first.resolved_identity.unwrap();
    let previous = harness
        .store
        .edit(NAMESPACE, ACCOUNT, SCOPE, move |editor| {
            editor.wire_from_upstream_response(
                "resp_prewarm",
                Some(&WireIdOwner {
                    session_id: identity.session_id.clone(),
                    thread_id: identity.thread_id.clone(),
                }),
            )
        })
        .await
        .unwrap();
    let mut foreign = body;
    foreign["previous_response_id"] = previous.into();
    assert!(matches!(
        prepare(&harness.store, "other-key", foreign).await,
        Err(StatefulPrepareError::StateUnavailable)
    ));
    harness
        .store
        .edit(NAMESPACE, ACCOUNT, SCOPE, |editor| {
            assert!(
                editor
                    .existing_wire_from_downstream(WireIdDomain::Turn, "shared-turn")?
                    .is_some()
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn history_prewarm_keeps_native_lite_prefix_derivation_and_metadata_shape() {
    let harness = CodexStateTestHarness::new();
    let thread = "native-history-session";
    let namespace = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, thread.as_bytes());
    let base = "Synthetic caller base";
    let input = json!([
        {"type":"additional_tools","role":"developer","tools":[],
            "id":format!("at_{}", uuid::Uuid::new_v5(&namespace, b"[]"))},
        {"type":"message","role":"developer","content":[{"type":"input_text","text":base}],
            "id":format!("msg_{}", uuid::Uuid::new_v5(&namespace, base.as_bytes()))},
        message("user", "native-history-turn"), message("assistant", "native-history-turn")
    ]);
    let mut body = request("gpt-6-sol", thread, "", input.clone(), true);
    // A formed native Lite request carries its base only in the input prefix.
    body.as_object_mut().unwrap().remove("instructions");
    let first = prepare(&harness.store, SCOPE, body.clone()).await.unwrap();
    let second = prepare(&harness.store, SCOPE, body).await.unwrap();
    assert_empty_turn(&first);
    let wire = value(&first);
    assert_eq!(wire["input"], value(&second)["input"]);
    assert_eq!(wire["input"].as_array().unwrap().len(), 4);
    let namespace = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        first
            .resolved_identity
            .as_ref()
            .unwrap()
            .thread_id
            .as_bytes(),
    );
    assert_eq!(
        wire["input"][0]["id"],
        format!("at_{}", uuid::Uuid::new_v5(&namespace, b"[]"))
    );
    assert_eq!(
        wire["input"][1]["id"],
        format!("msg_{}", uuid::Uuid::new_v5(&namespace, base.as_bytes()))
    );
    for index in 0..2 {
        assert_ne!(wire["input"][index]["id"], input[index]["id"]);
        assert!(wire["input"][index].get(META).is_none());
    }
    assert!(
        !wire["input"][2][META]["turn_id"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        wire["input"][2][META]["turn_id"],
        wire["input"][3][META]["turn_id"]
    );
}
