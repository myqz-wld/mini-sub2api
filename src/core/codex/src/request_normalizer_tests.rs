use super::*;
use pretty_assertions::assert_eq;

const PSEUDONYM_SCOPE: &str = "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

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
        PSEUDONYM_SCOPE,
        "req_test",
    )
    .expect("normalized request");
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
    assert!(value["input"][2].get("id").is_none());
    assert_eq!(
        value["input"][1]["content"][0]["text"],
        crate::codex_instructions::for_model("gpt-5.6-sol")
    );
    assert_eq!(value["input"][2]["content"][0]["text"], "Be concise");
    assert_eq!(value["input"][3]["role"], "developer");
    assert_eq!(
        value["input"][3]["content"],
        serde_json::json!([{"type":"input_text","text":"Follow system rules"}])
    );
    assert_eq!(value["input"][4]["role"], "user");
    let turn_id = value["client_metadata"]["turn_id"]
        .as_str()
        .expect("turn id");
    assert_ne!(turn_id, "turn-test");
    assert_eq!(
        uuid::Uuid::parse_str(turn_id)
            .expect("pseudonym UUID")
            .get_version_num(),
        8
    );
    for index in [3, 4] {
        assert!(
            value["input"][index]["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("msg_"))
        );
        assert_eq!(
            value["input"][index]["internal_chat_message_metadata_passthrough"]["turn_id"],
            turn_id
        );
        assert!(
            value["input"][index]["internal_chat_message_metadata_passthrough"]["create_time"]
                .is_number()
        );
    }
    assert_eq!(value["store"], true);
    assert_eq!(value["stream"], true);
    assert_eq!(value["tool_choice"], "auto");
    assert_eq!(value["parallel_tool_calls"], false);
    assert_eq!(value["reasoning"]["effort"], "low");
    assert_eq!(value["reasoning"]["context"], "all_turns");
    assert_eq!(value["text"]["verbosity"], "low");
    assert_ne!(value["client_metadata"]["session_id"], "session-test");
    assert!(
        uuid::Uuid::parse_str(
            value["client_metadata"]["x-codex-installation-id"]
                .as_str()
                .expect("installation id")
        )
        .is_ok()
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
    assert_eq!(
        value["prompt_cache_key"],
        value["client_metadata"]["session_id"]
    );
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
    let prepared = prepare_subscription_request(
        &headers,
        body,
        1024 * 1024,
        "acct_test",
        PSEUDONYM_SCOPE,
        "req_test",
    )
    .expect("normalized request");
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized JSON");

    assert_eq!(value["tools"][0]["name"], tools[0]["name"]);
    assert_eq!(value["tools"][0]["description"], "");
    assert_eq!(value["tools"][0]["strict"], false);
    assert_eq!(value["tools"][0]["parameters"], serde_json::json!({}));
    assert_eq!(
        value["instructions"],
        crate::codex_instructions::for_model("gpt-5.4")
    );
    assert_eq!(value["input"][0]["role"], "developer");
    assert_eq!(value["input"][0]["content"][0]["text"], "Be concise");
    assert_eq!(value["input"][1]["role"], "developer");
    assert_eq!(value["input"][2]["role"], "user");
    assert_eq!(value["input"][2]["type"], "message");
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
fn strips_subscription_output_cap_sampling_and_unknown_fields() {
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
        PSEUDONYM_SCOPE,
        "req_test",
    )
    .expect("normalized request");
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized JSON");

    for field in [
        "max_output_tokens",
        "max_completion_tokens",
        "max_tokens",
        "temperature",
        "top_p",
        "frequency_penalty",
        "presence_penalty",
        "future_request_field",
    ] {
        assert!(value.get(field).is_none(), "field {field} crossed");
    }
    assert_eq!(
        value["stream_options"]["reasoning_summary_delivery"],
        "sequential_cutoff"
    );
    assert_eq!(value["service_tier"], "auto");
    assert_eq!(value["reasoning"]["effort"], "medium");
}

#[test]
fn filters_unsupported_fields_from_already_subscription_shaped_json() {
    let body = Bytes::from_static(
        br#"{"model":"gpt-5.6-sol","input":[{"type":"additional_tools","role":"developer","tools":[]}],"stream":true,"max_output_tokens":64,"prompt_cache_key":"session-test"}"#,
    );
    let prepared = prepare_subscription_request(
        &HeaderMap::new(),
        body,
        64 * 1024,
        "acct_test",
        PSEUDONYM_SCOPE,
        "req_test",
    )
    .expect("normalized request");
    let value: Value = serde_json::from_slice(&prepared.body).expect("filtered JSON");

    assert!(value.get("max_output_tokens").is_none());
    let cache_key = value["prompt_cache_key"].as_str().expect("cache key");
    assert_ne!(cache_key, "session-test");
    assert_eq!(
        uuid::Uuid::parse_str(cache_key)
            .expect("pseudonym UUID")
            .get_version_num(),
        8
    );
    assert_eq!(value["input"][0]["type"], "additional_tools");
    assert_eq!(
        value["input"][1]["content"][0]["text"],
        crate::codex_instructions::for_model("gpt-5.6-sol")
    );
}

#[test]
fn incomplete_native_request_is_enriched_and_encoded_body_fails_closed() {
    let native = Bytes::from_static(
        br#"{"model":"gpt-5.6-sol","input":[{"type":"additional_tools","role":"developer","tools":[]}],"stream":true}"#,
    );
    let native_prepared = prepare_subscription_request(
        &HeaderMap::new(),
        native.clone(),
        64 * 1024,
        "acct_test",
        PSEUDONYM_SCOPE,
        "req_test",
    )
    .expect("normalized request");
    assert_ne!(native_prepared.body, native);
    let enriched: Value = serde_json::from_slice(&native_prepared.body).expect("enriched request");
    assert!(
        uuid::Uuid::parse_str(
            enriched["client_metadata"]["x-codex-installation-id"]
                .as_str()
                .expect("installation id")
        )
        .is_ok()
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
        encoded,
        1024,
        "acct_test",
        PSEUDONYM_SCOPE,
        "req_test",
    );
    assert!(prepared.is_err());
}

#[test]
fn complete_codex_request_pseudonymizes_identity_deterministically() {
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
        PSEUDONYM_SCOPE,
        "request-test",
    )
    .expect("normalized request");
    assert_ne!(prepared.body, body);
    let repeated = prepare_subscription_request(
        &headers,
        body,
        16 * 1024,
        "11111111-1111-4111-8111-111111111111",
        PSEUDONYM_SCOPE,
        "request-test",
    )
    .expect("normalized request");
    assert_eq!(prepared.body, repeated.body);
    assert_eq!(prepared.headers, repeated.headers);
    let projected_thread = header_text(&prepared.headers, "thread-id").expect("thread id");
    let projected_client =
        header_text(&prepared.headers, "x-client-request-id").expect("client request id");
    assert_ne!(projected_client, projected_thread);
    assert_eq!(
        uuid::Uuid::parse_str(&projected_client)
            .expect("client request UUID")
            .get_version_num(),
        8
    );
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized JSON");
    for (name, raw) in [
        ("session_id", "session-test"),
        ("thread_id", "thread-test"),
        ("turn_id", "turn-test"),
        (
            "x-codex-installation-id",
            "11111111-1111-4111-8111-111111111111",
        ),
    ] {
        let pseudonym = value["client_metadata"][name].as_str().expect("pseudonym");
        assert_ne!(pseudonym, raw);
        assert_eq!(
            uuid::Uuid::parse_str(pseudonym)
                .expect("pseudonym UUID")
                .get_version_num(),
            8
        );
    }
    assert_eq!(
        value["input"][0]["internal_chat_message_metadata_passthrough"]["turn_id"],
        value["client_metadata"]["turn_id"]
    );
}
