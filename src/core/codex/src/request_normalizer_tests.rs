use super::*;
use pretty_assertions::assert_eq;

#[test]
fn normalizes_responses_lite_with_codex_namespace_and_identity_shape() {
    let mut headers = HeaderMap::new();
    headers.insert("session-id", "session-test".parse().expect("header"));
    headers.insert("thread-id", "thread-test".parse().expect("header"));
    headers.insert(
        "x-codex-turn-metadata",
        r#"{"turn_id":"turn-test","sandbox":"none","sandbox_mode":"danger-full-access"}"#
            .parse()
            .expect("header"),
    );
    headers.insert(
        "x-openai-internal-codex-responses-lite",
        "false".parse().expect("header"),
    );
    let tools = serde_json::json!([
        {"type":"function","name":"lookup","description":"Lookup","parameters":{"type":"object"}},
        {"type":"web_search_preview"}
    ]);
    let body = serde_json::to_vec(&serde_json::json!({
        "model": "gpt-5.6-sol",
        "instructions": "Be concise",
        "input": [
            {"type":"message","role":"system","content":"Follow system rules"},
            {"type":"message","role":"user","content":"hello"}
        ],
        "tools": tools,
        "stream": true,
        "store": true
    }))
    .expect("request");

    let prepared = prepare_subscription_request(
        &headers,
        Bytes::from(body),
        1024 * 1024,
        "acct_test",
        "req_test",
    );
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized JSON");
    let ordered_fields = value
        .as_object()
        .expect("request object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(
        ordered_fields,
        [
            "model",
            "input",
            "tool_choice",
            "parallel_tool_calls",
            "reasoning",
            "store",
            "stream",
            "include",
            "prompt_cache_key",
            "text",
            "client_metadata",
        ]
    );

    assert!(value.get("tools").is_none());
    assert!(value.get("instructions").is_none());
    assert_eq!(value["input"][0]["type"], "additional_tools");
    assert_eq!(value["input"][0]["tools"][0]["type"], "namespace");
    assert_eq!(value["input"][0]["tools"][0]["name"], "functions");
    assert_eq!(value["input"][0]["tools"][0]["tools"][0]["name"], "lookup");
    assert_eq!(value["input"][0]["tools"][1], tools[1]);
    assert!(value["input"][0].get("id").is_none());
    assert_eq!(value["input"][1]["role"], "developer");
    assert!(value["input"][1].get("id").is_none());
    assert_eq!(value["input"][2]["role"], "developer");
    assert_eq!(
        value["input"][2]["content"],
        serde_json::json!([{"type":"input_text","text":"Follow system rules"}])
    );
    assert_eq!(value["input"][3]["role"], "user");
    for index in [2, 3] {
        assert!(
            value["input"][index]["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("msg_"))
        );
        assert_eq!(
            value["input"][index]["internal_chat_message_metadata_passthrough"]["turn_id"],
            "turn-test"
        );
        assert!(
            value["input"][index]["internal_chat_message_metadata_passthrough"]["create_time"]
                .is_number()
        );
    }
    assert_eq!(value["store"], false);
    assert_eq!(value["stream"], true);
    assert_eq!(value["tool_choice"], "auto");
    assert_eq!(value["parallel_tool_calls"], false);
    assert_eq!(value["reasoning"]["effort"], "low");
    assert_eq!(value["reasoning"]["context"], "all_turns");
    assert_eq!(value["text"]["verbosity"], "low");
    assert_eq!(value["client_metadata"]["session_id"], "session-test");
    assert_eq!(value["client_metadata"]["turn_id"], "turn-test");
    assert_eq!(
        value["client_metadata"]["x-codex-installation-id"],
        "acct_test"
    );
    let turn_metadata: Value = serde_json::from_str(
        value["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("turn metadata"),
    )
    .expect("turn metadata JSON");
    let metadata_fields = turn_metadata
        .as_object()
        .expect("turn metadata object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert!(
        metadata_fields.iter().position(|name| *name == "sandbox")
            < metadata_fields
                .iter()
                .position(|name| *name == "auto_review_enabled")
    );
    assert!(turn_metadata.get("tool_namespaces_info").is_none());
    assert_eq!(value["prompt_cache_key"], "session-test");
    assert_eq!(
        header_text(&prepared.headers, "x-openai-internal-codex-responses-lite").as_deref(),
        Some("true")
    );
    assert_eq!(
        header_text(&prepared.headers, "x-codex-routing-hint").as_deref(),
        Some("model=gpt-5.6-sol")
    );
    assert!(!prepared.headers.contains_key("x-codex-installation-id"));
    assert_eq!(
        header_text(&prepared.headers, "x-codex-beta-features").as_deref(),
        Some("remote_compaction_v2")
    );
}

#[test]
fn normalizes_non_lite_with_current_model_defaults() {
    let tools = serde_json::json!([{"type":"function","name":"lookup"}]);
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "gpt-5.4",
            "instructions": "Be concise",
            "input": [
                {"type":"message","role":"system","content":"Follow system rules"},
                {"role":"user","content":"hello"}
            ],
            "tools": tools
        }))
        .expect("request"),
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-openai-internal-codex-responses-lite",
        "true".parse().expect("header"),
    );
    let prepared =
        prepare_subscription_request(&headers, body, 1024 * 1024, "acct_test", "req_test");
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized JSON");

    assert_eq!(value["tools"][0]["name"], tools[0]["name"]);
    assert_eq!(value["tools"][0]["description"], "");
    assert_eq!(value["tools"][0]["strict"], false);
    assert_eq!(value["tools"][0]["parameters"], serde_json::json!({}));
    assert_eq!(value["instructions"], "Be concise");
    assert_eq!(value["input"][0]["role"], "developer");
    assert_eq!(value["input"][1]["role"], "user");
    assert_eq!(value["input"][1]["type"], "message");
    assert_eq!(value["parallel_tool_calls"], true);
    assert_eq!(value["reasoning"]["effort"], "medium");
    assert!(value["reasoning"].get("context").is_none());
    assert_eq!(value["text"]["verbosity"], "low");
    assert_eq!(value["stream"], true);
    assert_eq!(
        header_text(&prepared.headers, "x-client-request-id"),
        header_text(&prepared.headers, "thread-id")
    );
    let thread_id = header_text(&prepared.headers, "thread-id").expect("thread id");
    assert_eq!(
        uuid::Uuid::parse_str(&thread_id)
            .expect("thread UUID")
            .get_version_num(),
        7
    );
    assert_eq!(
        header_text(&prepared.headers, "x-codex-window-id"),
        Some(format!("{thread_id}:0"))
    );
    assert!(
        !prepared
            .headers
            .contains_key("x-openai-internal-codex-responses-lite")
    );
}

#[test]
fn strips_unsupported_output_and_sampling_fields_from_subscription_requests() {
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "gpt-5.6-luna",
            "input": "hello",
            "tools": [],
            "max_output_tokens": 32768,
            "max_completion_tokens": 16384,
            "max_tokens": 8192,
            "temperature": 0.2,
            "top_p": 0.9,
            "frequency_penalty": 0.1,
            "presence_penalty": 0.1,
            "future_request_field": true,
            "stream_options": {"reasoning_summary_delivery": "sequential_cutoff"},
            "service_tier": "auto"
        }))
        .expect("request"),
    );
    let prepared = prepare_subscription_request(
        &HeaderMap::new(),
        body,
        1024 * 1024,
        "acct_test",
        "req_test",
    );
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized JSON");

    for field in UNSUPPORTED_SUBSCRIPTION_BODY_FIELDS {
        assert!(value.get(*field).is_none(), "field {field} crossed");
    }
    assert!(value.get("future_request_field").is_none());
    assert_eq!(
        value["stream_options"]["reasoning_summary_delivery"],
        "sequential_cutoff"
    );
    assert_eq!(value["service_tier"], "auto");
    assert_eq!(value["reasoning"]["effort"], "medium");
}

#[test]
fn strips_unsupported_fields_from_already_subscription_shaped_json() {
    let body = Bytes::from_static(
        br#"{"model":"gpt-5.6-sol","input":[{"type":"additional_tools","role":"developer","tools":[]}],"stream":true,"max_output_tokens":64,"prompt_cache_key":"session-test"}"#,
    );
    let prepared =
        prepare_subscription_request(&HeaderMap::new(), body, 1024, "acct_test", "req_test");
    let value: Value = serde_json::from_slice(&prepared.body).expect("filtered JSON");

    assert!(value.get("max_output_tokens").is_none());
    assert_eq!(value["prompt_cache_key"], "session-test");
    assert_eq!(value["input"][0]["type"], "additional_tools");
}

#[test]
fn incomplete_native_request_is_enriched_but_encoded_body_remains_exact() {
    let native = Bytes::from_static(
        br#"{"model":"gpt-5.6-sol","input":[{"type":"additional_tools","role":"developer","tools":[]}],"stream":true}"#,
    );
    let native_prepared = prepare_subscription_request(
        &HeaderMap::new(),
        native.clone(),
        64 * 1024,
        "acct_test",
        "req_test",
    );
    assert_ne!(native_prepared.body, native);
    let enriched: Value = serde_json::from_slice(&native_prepared.body).expect("enriched request");
    assert_eq!(
        enriched["client_metadata"]["x-codex-installation-id"],
        "acct_test"
    );
    for name in [
        "session-id",
        "thread-id",
        "x-client-request-id",
        "x-codex-turn-metadata",
        "x-codex-window-id",
    ] {
        assert!(native_prepared.headers.contains_key(name), "missing {name}");
    }
    assert_eq!(
        header_text(
            &native_prepared.headers,
            "x-openai-internal-codex-responses-lite"
        )
        .as_deref(),
        Some("true")
    );
    assert!(
        !native_prepared
            .headers
            .contains_key("x-codex-installation-id")
    );

    let mut encoded_headers = HeaderMap::new();
    encoded_headers.insert(
        http::header::CONTENT_ENCODING,
        "zstd".parse().expect("encoding"),
    );
    let encoded = Bytes::from_static(b"compressed bytes");
    let prepared = prepare_subscription_request(
        &encoded_headers,
        encoded.clone(),
        1024,
        "acct_test",
        "req_test",
    );
    assert_eq!(prepared.body, encoded);
    assert_eq!(
        prepared.headers.get(http::header::CONTENT_ENCODING),
        encoded_headers.get(http::header::CONTENT_ENCODING)
    );
}

#[test]
fn complete_codex_request_keeps_canonical_body_bytes() {
    let turn_metadata = r#"{\"installation_id\":\"11111111-1111-4111-8111-111111111111\",\"session_id\":\"session-test\",\"thread_id\":\"thread-test\",\"agent_name\":\"/root\",\"turn_id\":\"turn-test\",\"window_id\":\"thread-test:0\",\"request_kind\":\"turn\",\"root_turn_id\":\"turn-test\",\"auto_review_enabled\":false,\"node_repl_auto_review_required\":false,\"node_repl_disabled\":false,\"turn_started_at_unix_ms\":1700000000000}"#;
    let body = Bytes::from(format!(
        r#"{{"model":"gpt-5.4","input":[{{"type":"message","id":"msg_11111111-1111-7111-8111-111111111111","role":"user","content":[{{"type":"input_text","text":"hello"}}],"internal_chat_message_metadata_passthrough":{{"turn_id":"turn-test","create_time":1700000000.0}}}}],"tools":[],"tool_choice":"auto","parallel_tool_calls":true,"reasoning":{{"effort":"medium"}},"store":false,"stream":true,"include":["reasoning.encrypted_content"],"prompt_cache_key":"session-test","text":{{"verbosity":"low"}},"client_metadata":{{"session_id":"session-test","thread_id":"thread-test","turn_id":"turn-test","x-codex-installation-id":"11111111-1111-4111-8111-111111111111","x-codex-turn-metadata":"{turn_metadata}","x-codex-window-id":"thread-test:0","root_turn_id":"turn-test"}}}}"#
    ));
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
        ("x-client-request-id", "thread-test"),
        ("x-codex-window-id", "thread-test:0"),
    ] {
        headers.insert(name, value.parse().expect("header"));
    }
    let prepared = prepare_subscription_request(
        &headers,
        body.clone(),
        16 * 1024,
        "11111111-1111-4111-8111-111111111111",
        "request-test",
    );
    assert_eq!(prepared.body, body);
}

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
        "request-memory",
    );
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
fn gpt_5_2_defaults_reasoning_summary_and_omits_null_optionals() {
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
        "request-test",
    );
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized request");

    assert_eq!(value["reasoning"]["effort"], "medium");
    assert_eq!(value["reasoning"]["summary"], "auto");
    assert!(value["reasoning"].get("context").is_none());
    assert_eq!(value["text"]["verbosity"], "low");
    assert_eq!(value["tool_choice"], "auto");
    assert_eq!(value["parallel_tool_calls"], true);
    assert!(value.get("stream_options").is_none());
    assert!(value.get("service_tier").is_none());
    assert!(value.get("instructions").is_none());
    assert_eq!(
        value["client_metadata"]["x-codex-installation-id"],
        "installation-test"
    );
    assert_eq!(
        value["prompt_cache_key"],
        value["client_metadata"]["session_id"]
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
        "request-test",
    );
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized request");

    assert_eq!(value["reasoning"], serde_json::json!({"summary": "auto"}));
    assert!(value.get("text").is_none());
    assert_eq!(value["parallel_tool_calls"], true);
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
        "request-test",
    );
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
