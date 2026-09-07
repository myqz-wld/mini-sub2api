use super::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[path = "subscription_compaction_lite_tests.rs"]
mod lite_tests;

#[path = "subscription_checkpoint_tests.rs"]
mod checkpoint_tests;

fn compacted(text: &str) -> Value {
    json!({"type":"compaction","id":"cmp_checkpoint","encrypted_content":text})
}

fn message(role: &str, text: &str) -> Value {
    json!({"type":"message","role":role,"content":[{"type":if role == "assistant" {"output_text"} else {"input_text"},"text":text}]})
}

fn compact_request(items: Vec<Value>, lite: bool) -> Value {
    let mut items = items;
    if lite {
        items.insert(
            0,
            json!({"type":"additional_tools","role":"developer","tools":[]}),
        );
    }
    items.push(json!({"type":"compaction_trigger"}));
    let mut body = request(json!(items));
    if lite {
        body.as_object_mut().unwrap().remove("instructions");
    }
    body["client_metadata"] = json!({"session_id":"compact-session","x-codex-turn-metadata":
        json!({"session_id":"compact-session","request_kind":"compaction",
            "compaction":{"implementation":"responses_compaction_v2"}}).to_string()});
    body
}

async fn finish(
    store: &RequestStateStore,
    prepared: PreparedEmulatedRequest,
    output: Vec<Value>,
    done: bool,
) -> Value {
    finish_as(store, prepared, output, done, "resp_checkpoint").await
}

async fn finish_as(
    store: &RequestStateStore,
    prepared: PreparedEmulatedRequest,
    output: Vec<Value>,
    done: bool,
    id: &str,
) -> Value {
    let state = ResponseStateContext::new(
        OWNER,
        NAMESPACE,
        KEY,
        store,
        prepared.resolved_identity.as_ref(),
        prepared.pending_compaction.as_ref(),
    )
    .with_operation(prepared.operation);
    state
        .translate_value(json!({"type":"response.created","response":{"id":id}}))
        .await
        .unwrap();
    if done {
        for (index, item) in output.iter().enumerate() {
            state
                .translate_value(
                    json!({"type":"response.output_item.done","output_index":index,"item":item}),
                )
                .await
                .unwrap();
        }
    }
    let result = state
        .translate_value(json!({"type":"response.completed","response":{"id":id,"output":output}}))
        .await
        .unwrap();
    result["response"].clone()
}

fn delta(previous: &Value, items: Vec<Value>) -> Value {
    let mut body = request(json!(items));
    body["previous_response_id"] = previous["id"].clone();
    body
}

fn history(
    store: &RequestStateStore,
    response: &Value,
) -> Option<Arc<crate::subscription_index::History>> {
    store
        .contexts
        .inner
        .lock()
        .unwrap()
        .scopes
        .get(&ContextStore::scope_key(NAMESPACE, KEY))?
        .records
        .get(response["id"].as_str()?)?
        .history
        .clone()
}

#[tokio::test]
async fn explicit_compaction_keeps_caller_context_and_supports_two_http_deltas() {
    for lite in [false, true] {
        let (_temp, store) = store();
        let items = vec![
            message("system", "caller system"),
            message("developer", "caller rule"),
            message("user", "first user"),
            message("assistant", "discarded assistant"),
            json!({"type":"reasoning","encrypted_content":"discarded reasoning","summary":[]}),
            json!({"type":"function_call","id":"fc_old","call_id":"call_old","name":"probe","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"call_old","output":"discarded tool"}),
            json!({"type":"compaction","encrypted_content":"discarded checkpoint"}),
            message("user", "second user"),
        ];
        let first = prepare(&store, compact_request(items, lite)).await.unwrap();
        let completed = finish(&store, first, vec![compacted("opaque 新 checkpoint")], true).await;
        let cached = history(&store, &completed).expect("accepted compaction has a local window");
        assert!(
            cached.parent.is_none(),
            "replacement must release the old parent history"
        );
        let values = cached.values();
        let offset = usize::from(lite);
        assert_eq!(values.len(), offset + 5);
        assert!(values[offset] == message("system", "caller system"));
        assert!(values[offset + 1] == message("developer", "caller rule"));
        assert!(values[offset + 2] == message("user", "first user"));
        assert!(values[offset + 3] == message("user", "second user"));
        assert_eq!(
            values[offset + 4]["encrypted_content"],
            "opaque 新 checkpoint"
        );
        let second = prepare(&store, delta(&completed, vec![input("after compact")]))
            .await
            .unwrap();
        let wire: Value = serde_json::from_slice(&second.body).unwrap();
        assert!(wire.get("previous_response_id").is_none());
        assert!(
            !wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type"] == "compaction_trigger")
        );
        let next = publish(
            &store,
            second,
            "resp_followup",
            json!([message("assistant", "new answer")]),
        )
        .await;
        let third = prepare(&store, delta(&next, vec![input("another user")]))
            .await
            .unwrap();
        let wire: Value = serde_json::from_slice(&third.body).unwrap();
        assert_eq!(
            wire["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["type"] == "compaction")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn in_band_compaction_keeps_only_the_checkpoint_and_trailing_output() {
    let (_temp, store) = store();
    let first = prepare(&store, request(json!([input("covered input")])))
        .await
        .unwrap();
    let completed = finish(&store, first, vec![message("assistant", "covered output"),
        compacted("opaque inline"), message("assistant", "retained output"),
        json!({"type":"function_call","id":"fc_new","call_id":"call_new","name":"probe","arguments":"{}"})], true).await;
    let cached = history(&store, &completed).expect("in-band checkpoint has a local window");
    assert!(cached.parent.is_none());
    let values = cached.values();
    assert_eq!(values.len(), 3);
    assert_eq!(values[0]["type"], "compaction");
    assert!(values[1] == message("assistant", "retained output"));
    let body = delta(
        &completed,
        vec![
            json!({"type":"function_call_output","call_id":completed["output"][3]["call_id"],"output":"new tool result"}),
        ],
    );
    assert!(prepare(&store, body).await.is_ok());
}

#[tokio::test]
async fn ambiguous_compaction_windows_remain_unavailable() {
    for mode in [
        "unknown-input",
        "unresolved-call",
        "local-summary",
        "unobserved",
        "two-items",
        "nonterminal-trigger",
    ] {
        let (_temp, store) = store();
        let mut items = vec![input("source")];
        if mode == "unknown-input" {
            items.push(json!({"type":"future_context","data":"unknown"}));
        }
        if mode == "unresolved-call" {
            items.push(json!({"type":"function_call","call_id":"call_pending","name":"probe","arguments":"{}"}));
        }
        let mut body = compact_request(items, false);
        let mut output = vec![compacted("opaque")];
        if mode == "local-summary" {
            body["input"] = json!([input("source")]);
            body["client_metadata"]["x-codex-turn-metadata"] =
                json!({"request_kind":"compaction","compaction":{"implementation":"responses"}})
                    .to_string()
                    .into();
            output = vec![message("assistant", "client installs its own summary")];
        }
        if mode == "two-items" {
            output.push(compacted("another checkpoint"));
        }
        if mode == "nonterminal-trigger" {
            body["input"]
                .as_array_mut()
                .unwrap()
                .push(input("after trigger"));
        }
        let first = prepare(&store, body).await.unwrap();
        let completed = finish(&store, first, output, mode != "unobserved").await;
        assert!(
            history(&store, &completed).is_none(),
            "ambiguous mode {mode} created history"
        );
        assert!(matches!(
            prepare(&store, delta(&completed, vec![input("next")])).await,
            Err(StatefulPrepareError::StateUnavailable)
        ));
    }
}

#[tokio::test]
async fn compacted_away_item_references_cannot_reenter_a_full_request() {
    let (_temp, store) = store();
    let old = json!({"type":"message","id":"msg_discarded","role":"assistant","content":[{"type":"output_text","text":"old"}]});
    let first = prepare(&store, compact_request(vec![input("user"), old], false))
        .await
        .unwrap();
    let completed = finish(&store, first, vec![compacted("opaque")], true).await;
    assert!(history(&store, &completed).is_some());
    let mut full = history(&store, &completed).unwrap().values();
    full.push(json!({"type":"item_reference","id":"msg_discarded"}));
    let mut full = request(json!(full));
    full["client_metadata"] = json!({"session_id":"compact-session"});
    assert!(matches!(
        prepare(&store, full).await,
        Err(StatefulPrepareError::StateUnavailable)
    ));
    let invalid = delta(
        &completed,
        vec![json!({"type":"item_reference","id":"msg_discarded"})],
    );
    assert!(matches!(
        prepare(&store, invalid).await,
        Err(StatefulPrepareError::StateUnavailable)
    ));
    assert!(
        prepare(&store, delta(&completed, vec![input("valid next")]))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn compacted_history_obeys_scope_expiry_and_does_not_persist_bodies() {
    let (temp, store) = store();
    let first = prepare(
        &store,
        compact_request(vec![input("private source sentinel")], false),
    )
    .await
    .unwrap();
    let completed = finish(
        &store,
        first,
        vec![compacted("private encrypted sentinel")],
        true,
    )
    .await;
    assert!(history(&store, &completed).is_some());
    let body = delta(&completed, vec![input("next")]);
    let object = body.as_object().unwrap();
    let evidence = Evidence::read(object, &HeaderMap::new(), EmulationTransport::Http).unwrap();
    assert!(matches!(
        store.contexts.plan(
            ContextStore::scope_key(NAMESPACE, "another-key"),
            object,
            evidence,
            None,
            None
        ),
        Err(StatefulPrepareError::StateUnavailable)
    ));
    let bytes = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
    let persisted = String::from_utf8(bytes).unwrap();
    assert!(
        !persisted.contains("private source sentinel")
            && !persisted.contains("private encrypted sentinel")
    );
    {
        let mut inner = store.contexts.inner.lock().unwrap();
        for session in inner
            .scopes
            .get_mut(&ContextStore::scope_key(NAMESPACE, KEY))
            .unwrap()
            .sessions
            .values_mut()
        {
            session.last_business = Instant::now() - Duration::from_secs(3 * 60 * 60 + 1);
        }
    }
    assert!(matches!(
        prepare(&store, body).await,
        Err(StatefulPrepareError::StateUnavailable)
    ));
    assert!(
        prepare(&store, request(json!([input("complete replacement")])))
            .await
            .is_ok()
    );
    drop(temp);
}

#[tokio::test]
async fn compacted_window_capacity_failure_preserves_response_delivery() {
    let (_temp, mut store) = store();
    Arc::make_mut(&mut store.contexts.limits).output_bytes = 128 * 1024;
    let first = prepare(&store, compact_request(vec![input("source")], false))
        .await
        .unwrap();
    let opaque = "x".repeat(80 * 1024);
    let completed = finish(&store, first, vec![compacted(&opaque)], true).await;
    assert_eq!(
        completed["output"][0]["encrypted_content"]
            .as_str()
            .unwrap()
            .len(),
        opaque.len()
    );
    assert!(history(&store, &completed).is_none());
    assert!(matches!(
        prepare(&store, delta(&completed, vec![input("next")])).await,
        Err(StatefulPrepareError::StateUnavailable)
    ));
}

#[tokio::test]
async fn a_checkpoint_cannot_complete_missing_local_source_history() {
    let (_temp, store) = store();
    let first = prepare(&store, compact_request(vec![input("source")], false))
        .await
        .unwrap();
    // Model an operation continuing entirely through an upstream WS reference.
    let operation = first.operation.as_ref().unwrap();
    store
        .contexts
        .inner
        .lock()
        .unwrap()
        .operations
        .get_mut(&operation.0.id)
        .unwrap()
        .record
        .history = None;
    let completed = finish(&store, first, vec![compacted("opaque")], true).await;
    assert!(history(&store, &completed).is_none());
}

#[tokio::test]
async fn repeated_compaction_uses_the_referenced_window_not_the_latest_session() {
    let (_temp, store) = store();
    let first = prepare(
        &store,
        compact_request(vec![input("original branch")], false),
    )
    .await
    .unwrap();
    let one = finish(&store, first, vec![compacted("checkpoint one")], true).await;
    let mut other = request(json!([input("unrelated latest branch")]));
    other["client_metadata"] = json!({"session_id":"compact-session"});
    let other = prepare(&store, other).await.unwrap();
    publish(
        &store,
        other,
        "resp_other",
        json!([message("assistant", "other answer")]),
    )
    .await;
    let mut again = compact_request(vec![input("original branch suffix")], false);
    again["previous_response_id"] = one["id"].clone();
    let second = prepare(&store, again).await.unwrap();
    let two = finish_as(
        &store,
        second,
        vec![compacted("checkpoint two")],
        true,
        "resp_checkpoint_two",
    )
    .await;
    let values = history(&store, &two).unwrap().values();
    assert_eq!(values.len(), 3);
    assert!(values[0] == message("user", "original branch"));
    assert!(values[1] == message("user", "original branch suffix"));
    assert_eq!(values[2]["encrypted_content"], "checkpoint two");
    assert_eq!(
        history(&store, &one).unwrap().values().last().unwrap()["encrypted_content"],
        "checkpoint one"
    );
    assert!(
        prepare(
            &store,
            delta(&two, vec![input("continue second checkpoint")])
        )
        .await
        .is_ok()
    );
}
