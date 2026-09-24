use super::*;

#[tokio::test]
async fn final_http_peer_observes_role_and_schema_policy() {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", post(context_upstream))
            .with_state(captures.clone()),
    )
    .await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    let schema: Value = serde_json::from_str(r#"{"type":"object","properties":{"z":{"const":123456789012345678901234567890},"a":{"type":"string"}}}"#).unwrap();
    let body = json!({"model":"gpt-5.4","input":[user("synthetic")],"tools":[
        {"type":"function","name":"lookup","parameters":{"type":"object","properties":{"x":{"const":"fixed"},"enum_value":{"enum":[{"const":"business"}]}}}},
        {"type":"mcp"}],"max_tool_calls":3,"background":true,"prompt":{},"top_logprobs":5,
        "tool_choice":"required","include":["message.output_text.logprobs"],"service_tier":"default",
        "reasoning":{"effort":"ultra","summary":"none"},"text":{"format":{"type":"json_schema","name":"caller","strict":false,"schema":schema}}});
    request(&state, &account, body, HeaderMap::new()).await;
    let captures = captures.lock().await;
    let value = &captures[0];
    for key in [
        "max_tool_calls",
        "background",
        "prompt",
        "top_logprobs",
        "service_tier",
    ] {
        assert!(value.get(key).is_none());
    }
    assert_eq!(value["tool_choice"], "auto");
    assert_eq!(value["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(value["reasoning"], json!({"effort":"xhigh"}));
    assert_eq!(value["tools"].as_array().unwrap().len(), 1);
    let props = &value["tools"][0]["parameters"]["properties"];
    assert_eq!(props["x"]["enum"], json!(["fixed"]));
    assert!(props["x"].get("const").is_none());
    assert_eq!(props["enum_value"]["enum"][0]["const"], "business");
    assert_eq!(value["text"]["format"]["name"], "codex_output_schema");
    assert_eq!(value["text"]["format"]["strict"], true);
    assert_eq!(
        serde_json::to_string(&value["text"]["format"]["schema"]).unwrap(),
        serde_json::to_string(&schema).unwrap()
    );
}

#[tokio::test]
async fn item_references_are_ownership_checked_before_omission_at_the_peer() {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", post(context_upstream))
            .with_state(captures.clone()),
    )
    .await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    let first = request(
        &state,
        &account,
        json!({"model":"gpt-5.4","input":[user("first")]}),
        HeaderMap::new(),
    )
    .await;
    request(
        &state,
        &account,
        json!({"model":"gpt-5.4","input":[user("first"),first["output"][0],
        {"type":"item_reference","id":first["output"][0]["id"]},user("next")]}),
        HeaderMap::new(),
    )
    .await;
    let wire = captures.lock().await;
    assert_eq!(wire.len(), 2);
    assert!(
        wire[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["type"] != "item_reference")
    );
    drop(wire);
    let invalid = call_core_with_headers(
        &state,
        &account,
        Bytes::from_static(
            br#"{"model":"gpt-5.4","input":[{"type":"item_reference","id":"msg_unowned"}]}"#,
        ),
        HeaderMap::new(),
    )
    .await;
    assert!(matches!(invalid, Err(CoreFailure::StateUnavailable)));
    assert_eq!(captures.lock().await.len(), 2);
}

#[tokio::test]
async fn final_http_peer_keeps_guardian_roles_separate() {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", post(context_upstream))
            .with_state(captures.clone()),
    )
    .await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    let parent = request(
        &state,
        &account,
        json!({"model":"gpt-5.4","input":[user("parent")],
        "client_metadata":{"session_id":"root","thread_id":"root","turn_id":"root-turn"}}),
        HeaderMap::new(),
    )
    .await;
    for role in ["classifier", "reviewer"] {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-guardian", role.parse().unwrap());
        headers.insert("x-openai-subagent", "guardian".parse().unwrap());
        headers.insert("x-codex-parent-thread-id", "root".parse().unwrap());
        request(&state,&account,json!({"model":"gpt-5.6-luna","input":[user("review")],
            "include":[],"tool_choice":"none","service_tier":"default",
            "text":{"format":{"type":"json_schema","name":"caller","strict":false,"schema":{"type":"object"}}},
            "client_metadata":{"session_id":"root","thread_id":role,"turn_id":format!("{role}-turn"),"parent_thread_id":"root","parent_response_id":parent["id"]}}),headers).await;
    }
    let wire = captures.lock().await;
    assert_eq!(
        wire[1]["client_metadata"]["parent_response_id"],
        "resp_context_1"
    );
    assert_eq!(wire[1]["tool_choice"], "none");
    assert_eq!(wire[1]["include"], json!([]));
    assert!(wire[1].get("text").is_none());
    assert_eq!(wire[2]["tool_choice"], "auto");
    assert_eq!(wire[2]["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(wire[2]["text"]["format"]["strict"], false);
    assert_eq!(wire[2]["text"]["format"]["name"], "codex_output_schema");
    assert!(wire[1].get("service_tier").is_none());
    assert!(wire[2].get("service_tier").is_none());
}

#[tokio::test]
async fn malformed_tool_schema_fails_before_delivery_but_opaque_enum_objects_remain_valid() {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", post(context_upstream))
            .with_state(captures.clone()),
    )
    .await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    for schema in [
        json!({"type":"array","minItems":-1}),
        json!({"type":"object","properties":[]}),
        json!({"type":"object","required":[1]}),
        json!({"type":"string","encrypted":"bad"}),
        json!(7),
    ] {
        let body = json!({"model":"gpt-5.4","input":[user("synthetic")],"tools":[{"type":"function","name":"test","parameters":schema}]});
        let result = call_core_with_headers(
            &state,
            &account,
            Bytes::from(body.to_string()),
            HeaderMap::new(),
        )
        .await;
        assert!(matches!(result, Err(CoreFailure::InvalidRequest)));
    }
    assert!(captures.lock().await.is_empty());
}
