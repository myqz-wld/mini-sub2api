use super::*;
use std::assert_eq;

#[tokio::test]
async fn missing_turn_reuses_for_tool_roundtrip_and_changes_for_new_user() {
    let (_temp, store) = store();
    let headers = HeaderMap::new();
    let request = |input: Value| {
        serde_json::json!({
            "model":"gpt-5.4",
            "input":input,
            "client_metadata":{"session_id":"conversation-stable"}
        })
    };
    let first_input = serde_json::json!([{
        "type":"message","role":"user","content":[{"type":"input_text","text":"first"}]
    }]);
    let first = value(&prepare(&store, &headers, request(first_input.clone())).await);
    let first_turn = first["client_metadata"]["turn_id"]
        .as_str()
        .expect("first turn")
        .to_string();
    let first_user = first["input"]
        .as_array()
        .expect("input")
        .iter()
        .find(|item| item["role"] == "user")
        .expect("user");
    assert!(first_user.get("id").is_none());
    let first_create_time =
        first_user["internal_chat_message_metadata_passthrough"]["create_time"].clone();

    let call = seed_upstream_wire(&store, WireIdDomain::Call, "call_provider").await;

    let tool_input = serde_json::json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]},
        {"type":"function_call_output","call_id":call,"output":"done"}
    ]);
    let tool = value(&prepare(&store, &headers, request(tool_input)).await);
    assert_eq!(tool["client_metadata"]["turn_id"], first_turn);
    let repeated_user = tool["input"]
        .as_array()
        .expect("input")
        .iter()
        .find(|item| item["role"] == "user")
        .expect("user");
    assert!(repeated_user.get("id").is_none());
    assert_eq!(
        repeated_user["internal_chat_message_metadata_passthrough"]["create_time"],
        first_create_time
    );

    let minimal_call =
        seed_upstream_wire(&store, WireIdDomain::Call, "call_provider_minimal").await;
    let minimal_tool = value(
        &prepare(
            &store,
            &headers,
            request(serde_json::json!([{
                "type":"function_call_output",
                "call_id":minimal_call,
                "output":"done"
            }])),
        )
        .await,
    );
    assert_eq!(minimal_tool["client_metadata"]["turn_id"], first_turn);

    let next_input = serde_json::json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]},
        {"type":"function_call_output","call_id":call,"output":"done"},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"second"}]}
    ]);
    let next = value(&prepare(&store, &headers, request(next_input)).await);
    assert_ne!(next["client_metadata"]["turn_id"], first_turn);
}

#[tokio::test]
async fn explicit_parent_lineage_keeps_root_session_and_distinct_child_thread() {
    let (_temp, store) = store();
    let body = serde_json::json!({
        "model":"gpt-5.4",
        "input":[{"type":"message","role":"user","content":"child work"}],
        "client_metadata":{
            "session_id":"root-session",
            "thread_id":"child-thread",
            "x-codex-turn-metadata":"{\"parent_thread_id\":\"root-session\",\"turn_id\":\"child-turn\",\"root_turn_id\":\"root-turn\"}"
        }
    });
    let prepared = prepare(&store, &HeaderMap::new(), body).await;
    let value = value(&prepared);
    let metadata = &value["client_metadata"];
    assert_ne!(metadata["session_id"], metadata["thread_id"]);
    assert_eq!(metadata["parent_thread_id"], metadata["session_id"],);
    assert_ne!(metadata["turn_id"], metadata["root_turn_id"]);
    assert_eq!(
        prepared.headers["x-codex-parent-thread-id"],
        metadata["session_id"].as_str().expect("session")
    );
}

#[tokio::test]
async fn oversized_schema_id_is_an_invalid_request_not_a_state_outage() {
    let (_temp, store) = store();
    let body = serde_json::json!({
        "model":"gpt-5.4",
        "input":"hello",
        "client_metadata":{"session_id":"x".repeat(513)}
    });
    let error = prepare_stateful_codex_request(
        UpstreamProfile::CodexSubscription1534,
        EmulationTransport::Http,
        &HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).expect("body JSON")),
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
            store: &store,
        },
        false,
    )
    .await
    .expect_err("oversized ID must fail");
    assert_eq!(error, StatefulPrepareError::InvalidRequest);
    assert!(!store.state_path_for_test(NAMESPACE).exists());
}

#[tokio::test]
async fn responses_conversation_does_not_select_session_and_carrier_free_calls_stay_distinct() {
    let (_temp, store) = store();
    let conversation_a =
        seed_upstream_wire(&store, WireIdDomain::Conversation, "conv-provider-a").await;
    let conversation_b =
        seed_upstream_wire(&store, WireIdDomain::Conversation, "conv-provider-b").await;
    let request = |conversation: Option<&str>, text: &str| {
        let mut value = serde_json::json!({
            "model":"gpt-5.4",
            "input":[{"type":"message","role":"user","content":text}]
        });
        if let Some(conversation) = conversation {
            value
                .as_object_mut()
                .expect("request")
                .insert("conversation".to_string(), conversation.into());
        }
        value
    };
    let a = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request(Some(&conversation_a), "same"),
        )
        .await,
    );
    let b = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request(Some(&conversation_b), "same"),
        )
        .await,
    );
    let a_next = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request(Some(&conversation_a), "different"),
        )
        .await,
    );
    assert_ne!(
        a["client_metadata"]["session_id"],
        b["client_metadata"]["session_id"]
    );
    assert_ne!(
        a["client_metadata"]["session_id"],
        a_next["client_metadata"]["session_id"]
    );

    let first_free = value(&prepare(&store, &HeaderMap::new(), request(None, "identical")).await);
    let second_free = value(&prepare(&store, &HeaderMap::new(), request(None, "identical")).await);
    assert_ne!(
        first_free["client_metadata"]["session_id"],
        second_free["client_metadata"]["session_id"]
    );
}

#[tokio::test]
async fn previous_response_alias_restores_its_conversation_and_thread_owner() {
    let (temp, store) = store();
    let first = prepare(
        &store,
        &HeaderMap::new(),
        serde_json::json!({
            "model":"gpt-5.4",
            "input":[{"type":"message","id":"msg_first","role":"user","content":"first"}]
        }),
    )
    .await;
    let first_value = value(&first);
    let first_identity = first.resolved_identity.as_ref().expect("identity");
    let response_state = ResponseStateContext::new(
        ACCOUNT_REF,
        NAMESPACE,
        SCOPE,
        &store,
        Some(first_identity),
        None,
    );
    let translated = response_state
        .translate_value(serde_json::json!({
            "type":"response.completed",
            "response":{"id":"resp_provider_first","output":[]}
        }))
        .await
        .expect("translate response");
    let alias = translated["response"]["id"]
        .as_str()
        .expect("response alias");
    assert_ne!(alias, "resp_provider_first");

    let reopened = RequestStateStore::new(temp.path().join("accounts"));
    let next = value(
        &prepare(
            &reopened,
            &HeaderMap::new(),
            serde_json::json!({
                "model":"gpt-5.4",
                "previous_response_id":alias,
                "input":[{"type":"message","id":"msg_next","role":"user","content":"next"}]
            }),
        )
        .await,
    );
    assert_eq!(
        next["client_metadata"]["session_id"],
        first_value["client_metadata"]["session_id"]
    );
    assert_eq!(
        next["client_metadata"]["thread_id"],
        first_value["client_metadata"]["thread_id"]
    );
    assert_ne!(
        next["client_metadata"]["turn_id"],
        first_value["client_metadata"]["turn_id"]
    );
}

#[tokio::test]
async fn equal_user_content_uses_item_identity_and_tool_history_reuses_active_turn() {
    let (_temp, store) = store();
    let request = |input: Value| {
        serde_json::json!({
            "model":"gpt-5.4",
            "input":input,
            "client_metadata":{"session_id":"equal-content-session"}
        })
    };
    let first = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request(serde_json::json!([{
                "type":"message","id":"msg_equal_1","role":"user","content":"repeat"
            }])),
        )
        .await,
    );
    let second_body = request(serde_json::json!([{
        "type":"message","id":"msg_equal_2","role":"user","content":"repeat"
    }]));
    let second = value(&prepare(&store, &HeaderMap::new(), second_body.clone()).await);
    let retry = value(&prepare(&store, &HeaderMap::new(), second_body).await);
    assert_ne!(
        first["client_metadata"]["turn_id"],
        second["client_metadata"]["turn_id"]
    );
    assert_eq!(
        second["client_metadata"]["turn_id"],
        retry["client_metadata"]["turn_id"]
    );

    let call = seed_upstream_wire(&store, WireIdDomain::Call, "call_provider_equal").await;
    let tool = value(
        &prepare(
            &store,
            &HeaderMap::new(),
            request(serde_json::json!([
                {"type":"message","id":"msg_equal_2","role":"user","content":"repeat"},
                {"type":"function_call_output","call_id":call,"output":"done"}
            ])),
        )
        .await,
    );
    assert_eq!(
        tool["client_metadata"]["turn_id"],
        second["client_metadata"]["turn_id"]
    );
}
