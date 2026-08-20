use super::*;
use pretty_assertions::assert_eq;

#[test]
fn normalizes_responses_lite_without_changing_tool_set() {
    let mut headers = HeaderMap::new();
    headers.insert("session-id", "session-test".parse().expect("header"));
    headers.insert("thread-id", "thread-test".parse().expect("header"));
    headers.insert(
        "x-codex-turn-metadata",
        r#"{"turn_id":"turn-test"}"#.parse().expect("header"),
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
        "input": "hello",
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

    assert!(value.get("tools").is_none());
    assert!(value.get("instructions").is_none());
    assert_eq!(value["input"][0]["type"], "additional_tools");
    assert_eq!(value["input"][0]["tools"], tools);
    assert_eq!(value["input"][1]["role"], "developer");
    assert_eq!(value["input"][2]["role"], "user");
    assert_eq!(value["store"], false);
    assert_eq!(value["stream"], true);
    assert_eq!(value["tool_choice"], "auto");
    assert_eq!(value["parallel_tool_calls"], false);
    assert_eq!(value["reasoning"]["effort"], "low");
    assert_eq!(value["reasoning"]["context"], "all_turns");
    assert_eq!(value["text"]["verbosity"], "low");
    assert_eq!(value["client_metadata"]["session_id"], "session-test");
    assert_eq!(value["client_metadata"]["turn_id"], "turn-test");
    assert_eq!(value["prompt_cache_key"], "session-test");
    assert_eq!(
        header_text(&prepared.headers, "x-openai-internal-codex-responses-lite").as_deref(),
        Some("true")
    );
}

#[test]
fn normalizes_non_lite_with_current_model_defaults() {
    let tools = serde_json::json!([{"type":"function","name":"lookup"}]);
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "gpt-5.4",
            "instructions": "Be concise",
            "input": "hello",
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

    assert_eq!(value["tools"], tools);
    assert_eq!(value["instructions"], "Be concise");
    assert_eq!(value["input"][0]["role"], "user");
    assert_eq!(value["parallel_tool_calls"], true);
    assert_eq!(value["reasoning"]["effort"], "medium");
    assert!(value["reasoning"].get("context").is_none());
    assert_eq!(value["text"]["verbosity"], "low");
    assert_eq!(value["stream"], true);
    assert_eq!(
        header_text(&prepared.headers, "x-client-request-id")
            .expect("client request id")
            .len(),
        36
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
fn native_and_encoded_requests_remain_byte_exact() {
    let native = Bytes::from_static(
        br#"{"model":"gpt-5.6-sol","input":[{"type":"additional_tools","role":"developer","tools":[]}],"stream":true}"#,
    );
    let native_prepared = prepare_subscription_request(
        &HeaderMap::new(),
        native.clone(),
        1024,
        "acct_test",
        "req_test",
    );
    assert_eq!(native_prepared.body, native);
    for name in [
        "session-id",
        "thread-id",
        "x-client-request-id",
        "x-codex-installation-id",
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
