use super::*;

const PSEUDONYM_SCOPE: &str = "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[test]
fn memory_request_preserves_sparse_turn_metadata_without_turn_identity() {
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "gpt-5.4",
            "input": "remember this",
            "tools": [],
            "client_metadata": {
                "session_id": "session-memory",
                "thread_id": "thread-memory",
                "x-codex-installation-id": "installation-memory",
                "x-codex-window-id": "thread-memory:0",
                "x-codex-turn-metadata": "{\"request_kind\":\"memory\",\"sandbox\":\"none\"}"
            }
        }))
        .expect("memory request"),
    );
    let prepared = prepare_subscription_request(
        &HeaderMap::new(),
        body,
        64 * 1024,
        "installation-memory",
        PSEUDONYM_SCOPE,
        "request-memory",
    )
    .expect("normalized memory request");
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized memory request");
    assert!(value["client_metadata"].get("turn_id").is_none());
    assert!(value["client_metadata"].get("root_turn_id").is_none());
    assert!(
        value["input"][0]
            .get("internal_chat_message_metadata_passthrough")
            .is_none()
    );
    assert_eq!(
        value["client_metadata"]["x-codex-turn-metadata"],
        "{\"request_kind\":\"memory\",\"sandbox\":\"none\"}"
    );
}

#[test]
fn gpt_5_2_preserves_explicit_null_public_members() {
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "gpt-5.2",
            "instructions": null,
            "input": "hello",
            "tools": null,
            "reasoning": {"effort": null, "summary": null, "context": null},
            "stream_options": null,
            "service_tier": null,
            "prompt_cache_key": null,
            "text": null,
            "client_metadata": null,
            "include": null,
            "tool_choice": null,
            "parallel_tool_calls": null
        }))
        .expect("request"),
    );
    let prepared = prepare_subscription_request(
        &HeaderMap::new(),
        body,
        64 * 1024,
        "installation-test",
        PSEUDONYM_SCOPE,
        "request-test",
    )
    .expect("normalized request");
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized request");

    assert!(value["reasoning"]["effort"].is_null());
    assert!(value["reasoning"]["summary"].is_null());
    assert!(value["reasoning"]["context"].is_null());
    assert!(value["text"].is_null());
    assert!(value["tool_choice"].is_null());
    assert!(value["parallel_tool_calls"].is_null());
    for name in [
        "instructions",
        "tools",
        "stream_options",
        "service_tier",
        "prompt_cache_key",
        "previous_response_id",
        "include",
    ] {
        assert!(value[name].is_null(), "field {name}");
    }
    assert!(
        uuid::Uuid::parse_str(
            value["client_metadata"]["x-codex-installation-id"]
                .as_str()
                .expect("installation id")
        )
        .is_ok()
    );
}

#[test]
fn unknown_model_uses_codex_fallback_reasoning_without_verbosity() {
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "custom-model",
            "input": "hello",
            "tools": []
        }))
        .expect("request"),
    );
    let prepared = prepare_subscription_request(
        &HeaderMap::new(),
        body,
        64 * 1024,
        "installation-test",
        PSEUDONYM_SCOPE,
        "request-test",
    )
    .expect("normalized request");
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized request");

    assert_eq!(value["reasoning"], serde_json::json!({"summary": "auto"}));
    assert!(value.get("text").is_none());
    assert_eq!(value["parallel_tool_calls"], true);
}

#[test]
fn derived_and_namespaced_models_use_catalog_profile_shape() {
    for (model, expected_verbosity, expected_lite) in [
        ("gpt-5.4-mini-preview", "medium", false),
        ("vendor/gpt-5.6-sol-snapshot", "low", true),
    ] {
        let body = Bytes::from(
            serde_json::to_vec(&serde_json::json!({
                "model": model,
                "input": "hello",
                "tools": []
            }))
            .expect("request"),
        );
        let prepared = prepare_subscription_request(
            &HeaderMap::new(),
            body,
            64 * 1024,
            "installation-test",
            PSEUDONYM_SCOPE,
            "request-test",
        )
        .expect("normalized request");
        let value: Value = serde_json::from_slice(&prepared.body).expect("normalized request");

        assert_eq!(value["text"]["verbosity"], expected_verbosity);
        assert_eq!(value["parallel_tool_calls"], !expected_lite);
        assert_eq!(
            value["input"][0]["type"] == "additional_tools",
            expected_lite
        );
        assert_eq!(value["reasoning"].get("context").is_some(), expected_lite);
    }
}

#[test]
fn reused_lite_websocket_frame_keeps_incremental_input_without_prefix() {
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "type": "response.create",
            "model": "gpt-5.6-sol",
            "previous_response_id": "resp_previous",
            "input": [],
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "reasoning": {"effort": "low", "context": "all_turns"},
            "store": false,
            "stream": true,
            "include": ["reasoning.encrypted_content"],
            "prompt_cache_key": "session-test",
            "text": {"verbosity": "low"},
            "client_metadata": {
                "session_id": "session-test",
                "thread_id": "thread-test",
                "turn_id": "turn-test"
            }
        }))
        .expect("request"),
    );
    let prepared = prepare_websocket_subscription_request(
        &HeaderMap::new(),
        body,
        64 * 1024,
        "installation-test",
        PSEUDONYM_SCOPE,
        "request-test",
    )
    .expect("normalized request");
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized request");

    assert_eq!(value["type"], "response.create");
    assert_eq!(value["previous_response_id"], "resp_previous");
    assert_eq!(value["input"], serde_json::json!([]));
    assert!(value.get("tools").is_none());
    assert!(value.get("instructions").is_none());
    assert_eq!(
        value["client_metadata"]["ws_request_header_x_openai_internal_codex_responses_lite"],
        "true"
    );
}
