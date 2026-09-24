use super::*;

#[path = "subscription_reasoning_boundary_tests.rs"]
mod boundaries;

#[path = "subscription_reasoning_lifecycle_tests.rs"]
mod lifecycle;

fn reasoning(id: &str, cipher: &str) -> Value {
    json!({"type":"reasoning","id":id,"summary":[{"type":"summary_text","text":"summary"}],"content":[{"type":"reasoning_text","text":"content"}],"encrypted_content":cipher})
}

fn assert_visibility(value: &Value, visible: bool) {
    let item = value.get("item").unwrap_or(&value["response"]["output"][0]);
    assert!(item["type"] == "reasoning", "reasoning fixture missing");
    assert!(
        item.get("encrypted_content").is_some() == visible,
        "reasoning visibility mismatch"
    );
    assert!(
        item["summary"][0]["text"] == "summary" && item["content"][0]["text"] == "content",
        "reasoning public text changed"
    );
}

#[tokio::test]
async fn include_preference_is_per_operation_and_filters_all_response_containers() {
    let (_temp, store) = store();
    let mut shared = None;
    for (index, include) in [
        None,
        Some(json!([])),
        Some(json!(["message.output_text.logprobs"])),
        Some(Value::Null),
        Some(json!([
            "message.output_text.logprobs",
            "reasoning.encrypted_content",
            "message.output_text.logprobs"
        ])),
    ]
    .into_iter()
    .enumerate()
    {
        let visible = include.as_ref().is_none_or(|value| {
            value.as_array().is_some_and(|values| {
                values
                    .iter()
                    .any(|value| value == "reasoning.encrypted_content")
            })
        });
        let mut body = request(json!([input("caller")]));
        if let Some(include) = &include {
            body["include"] = include.clone();
        }
        let prepared = prepare(&store, body).await.unwrap();
        let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert_eq!(wire["include"], json!(["reasoning.encrypted_content"]));
        let context = shared.get_or_insert_with(|| {
            ResponseStateContext::new(OWNER, NAMESPACE, KEY, &store, None, None)
        });
        context
            .update_identity(prepared.resolved_identity.as_ref())
            .unwrap();
        context
            .update_operation(prepared.operation.clone())
            .unwrap();
        let id = format!("resp_visibility_{index}");
        let item = reasoning(&format!("rs_visibility_{index}"), "private-state");
        for event in [
            json!({"type":"response.created","response":{"id":id,"output":[item]}}),
            json!({"type":"response.output_item.added","output_index":0,"item":item}),
            json!({"type":"response.output_item.done","output_index":0,"item":item}),
        ] {
            let returned = context.translate_value(event).await.unwrap();
            assert_visibility(&returned, visible);
        }
        let returned = if index % 2 == 0 {
            context
                .translate_value(
                    json!({"type":"response.completed","response":{"id":id,"output":[item]}}),
                )
                .await
                .unwrap()
        } else {
            json!({"response":context.translate_terminal_value(json!({"id":id,"output":[item]}),true).await.unwrap()})
        };
        assert_visibility(&returned, visible);
        let public_id = returned["response"]["id"].as_str().unwrap();
        let inner = store.contexts.inner.lock().unwrap();
        let scope = inner
            .scopes
            .get(&ContextStore::scope_key(NAMESPACE, KEY))
            .unwrap();
        let history = scope.records[public_id].history.as_ref().unwrap();
        assert!(
            history
                .values()
                .iter()
                .any(|v| v["encrypted_content"] == "private-state"),
            "private state was filtered before publication"
        );
        assert!(
            history.hidden_reasoning().is_empty() == visible,
            "hidden-field provenance disagrees with returned output"
        );
        let ledger = std::fs::read(store.state_path_for_test(NAMESPACE)).unwrap();
        assert!(
            !ledger
                .windows(b"private-state".len())
                .any(|window| window == b"private-state"),
            "private reasoning was persisted in the identity ledger"
        );
    }
}

#[tokio::test]
async fn malformed_include_fails_before_admission_without_affecting_null() {
    let (_temp, store) = store();
    for include in [
        json!(false),
        json!(1),
        json!("reasoning.encrypted_content"),
        json!({}),
        json!(["valid", 1]),
    ] {
        let mut body = request(json!([input("caller")]));
        body["include"] = include;
        assert!(
            matches!(
                prepare(&store, body).await,
                Err(StatefulPrepareError::InvalidRequest)
            ),
            "malformed include was accepted"
        );
    }
    assert!(store.contexts.inner.lock().unwrap().operations.is_empty());
}

#[test]
fn hiding_reasoning_preserves_compaction_and_opaque_tool_data() {
    let opaque = json!({"type":"reasoning","encrypted_content":"business-value"});
    let arguments = opaque.to_string();
    let mut response = json!({"response":{"output":[
        reasoning("rs_filter","private-state"),
        {"type":"compaction","encrypted_content":"checkpoint-state"},
        {"type":"function_call_output","output":opaque},
        {"type":"function_call","arguments":arguments}
    ]}});
    crate::reasoning_visibility::ReasoningVisibility::Hide.filter_response(&mut response);
    assert!(
        response["response"]["output"][0]
            .get("encrypted_content")
            .is_none()
    );
    assert!(response["response"]["output"][1]["encrypted_content"] == "checkpoint-state");
    assert!(response["response"]["output"][2]["output"] == opaque);
    assert!(response["response"]["output"][3]["arguments"].as_str() == Some(arguments.as_str()));
}

#[tokio::test]
async fn hidden_reasoning_stays_upstream_and_restores_anonymous_history() {
    for model in ["gpt-5.5", "gpt-5.6-sol"] {
        let (_temp, store) = store();
        let mut first = request(json!([input("first")]));
        first["model"] = json!(model);
        first["include"] = json!([]);
        let prepared = prepare(&store, first.clone()).await.unwrap();
        let identity = prepared.resolved_identity.as_ref().unwrap().clone();
        let wire: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert!(
            wire["include"] == json!(["reasoning.encrypted_content"]),
            "upstream encrypted include missing"
        );
        let response = publish(&store, prepared, "resp_hidden_one", json!([
            {"type":"reasoning","id":"rs_hidden_one","summary":[],"encrypted_content":"opaque-first"},
            {"type":"message","id":"msg_hidden_one","role":"assistant","content":[{"type":"output_text","text":"answer"}]}
        ])).await;
        assert!(
            response["output"][0].get("encrypted_content").is_none(),
            "unrequested reasoning ciphertext escaped"
        );
        let mut history = first["input"].as_array().unwrap().clone();
        history.extend(response["output"].as_array().unwrap().clone());
        history.push(input("next"));
        first["input"] = json!(history);
        let next = prepare(&store, first).await.unwrap();
        assert!(
            next.resolved_identity.as_ref().unwrap().session_id == identity.session_id,
            "redacted history lost its session"
        );
        let wire: Value = serde_json::from_slice(&next.body).unwrap();
        let reasoning = wire["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "reasoning")
            .unwrap();
        assert!(
            reasoning["encrypted_content"] == "opaque-first",
            "verified hidden reasoning was not restored"
        );
    }
}
