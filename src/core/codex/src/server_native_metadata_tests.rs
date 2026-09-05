use super::*;
use std::assert_eq;

#[tokio::test]
async fn native_1534_prefixes_are_stable_and_supported_per_response_controls_survive() {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", post(context_upstream))
            .with_state(captures.clone()),
    )
    .await;
    let (mut state, account, temp) = subscription_state(&upstream.base_url).await;
    let mut headers = HeaderMap::new();
    headers.insert("session-id", "stable-prefix-session".parse().unwrap());
    for program in ["standard", "daybreak_blue", "daybreak_red"] {
        if program == "daybreak_red" {
            // Reopen identity persistence with an empty memory cache, as after Core restart.
            state.vault = Vault::open(temp.path().to_path_buf()).expect("reopen vault");
        }
        request(&state, &account, json!({"model":"gpt-6-astra","instructions":"  caller {{personality}} base  ",
            "input":[user("first")], "stream":false, "tools":[{"type":"function","name":"example","parameters":{}}],
            "access_programs":{"cyber":program}, "stream_options":{"reasoning_summary_delivery":"sequential_cutoff"}
        }), headers.clone()).await;
    }
    let captures = captures.lock().await;
    for value in captures.iter() {
        let thread = value["client_metadata"]["thread_id"].as_str().unwrap();
        let namespace = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, thread.as_bytes());
        let tools = serde_json::to_vec(&value["input"][0]["tools"]).unwrap();
        let base = value["input"][1]["content"][0]["text"].as_str().unwrap();
        assert_eq!(base, "  caller {{personality}} base  ");
        assert_eq!(
            value["input"][0]["id"],
            format!("at_{}", uuid::Uuid::new_v5(&namespace, &tools))
        );
        assert_eq!(
            value["input"][1]["id"],
            format!("msg_{}", uuid::Uuid::new_v5(&namespace, base.as_bytes()))
        );
        assert_eq!(
            value["stream_options"]["reasoning_summary_delivery"],
            "sequential_cutoff"
        );
        assert_eq!(value["reasoning"]["context"], "all_turns");
    }
    assert_eq!(captures[0]["input"][0]["id"], captures[1]["input"][0]["id"]);
    assert_eq!(captures[0]["input"][1]["id"], captures[1]["input"][1]["id"]);
    assert_eq!(captures[1]["access_programs"]["cyber"], "daybreak_blue");
    assert_eq!(captures[0]["input"][0]["id"], captures[2]["input"][0]["id"]);
    assert_eq!(captures[0]["input"][1]["id"], captures[2]["input"][1]["id"]);
    assert_eq!(captures[2]["access_programs"]["cyber"], "daybreak_red");
}

#[tokio::test]
async fn unsupported_native_requirements_fail_before_inference() {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", post(context_upstream))
            .with_state(captures.clone()),
    )
    .await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    for member in [
        json!({"access_programs":{"cyber":"invented"}}),
        json!({"access_programs":{"cyber":"standard","extra":true}}),
        json!({"stream_options":{"reasoning_summary_delivery":"invented"}}),
        json!({"reasoning":{"context":"last_turn"}}),
    ] {
        let mut body = json!({"model":"gpt-6-astra","input":[user("first")]});
        body.as_object_mut()
            .unwrap()
            .extend(member.as_object().unwrap().clone());
        let failure = call_core_with_headers(
            &state,
            &account,
            Bytes::from(serde_json::to_vec(&body).unwrap()),
            HeaderMap::new(),
        )
        .await
        .expect_err("invalid native requirement");
        assert!(matches!(failure, CoreFailure::InvalidRequest));
    }
    assert_eq!(captures.lock().await.len(), 0);
}

#[tokio::test]
async fn native_window_metadata_is_validated_scoped_and_preserved_without_selecting_a_session() {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", post(context_upstream))
            .with_state(captures.clone()),
    )
    .await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    let raw_context = "01234567-89ab-4cde-8012-3456789abcde";
    let metadata = json!({"window_number":4,"context_window_id":raw_context,"turn_trigger":"user","history_ingest_requested":true});
    for _ in 0..2 {
        request(
            &state,
            &account,
            json!({"model":"gpt-5.4","instructions":"base","input":[user("first")], "stream":false,
            "client_metadata":{"x-codex-turn-metadata":metadata.to_string()}}),
            HeaderMap::new(),
        )
        .await;
    }
    let captures = captures.lock().await;
    assert_ne!(
        captures[0]["client_metadata"]["session_id"],
        captures[1]["client_metadata"]["session_id"]
    );
    let nested = |index: usize| {
        serde_json::from_str::<Value>(
            captures[index]["client_metadata"]["x-codex-turn-metadata"]
                .as_str()
                .unwrap(),
        )
        .unwrap()
    };
    for index in [0, 1] {
        let turn = nested(index);
        assert_eq!(turn["window_number"], 4);
        assert!(turn["window_id"].as_str().unwrap().ends_with(":4"));
        assert_ne!(turn["context_window_id"], raw_context);
        assert!(uuid::Uuid::parse_str(turn["context_window_id"].as_str().unwrap()).is_ok());
        assert_eq!(turn["turn_trigger"], "user");
        assert_eq!(turn["history_ingest_requested"], true);
    }
}

#[tokio::test]
async fn native_metadata_rejects_invalid_shapes_and_retains_fork_provenance_on_continuation() {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", post(context_upstream))
            .with_state(captures.clone()),
    )
    .await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    for turn in [
        json!({"window_number":-1}),
        json!({"window_number":2,"window_id":"thread:3"}),
        json!({"context_window_id":"invalid-uuid"}),
        json!({"forked_from_ordinal_exclusive":1}),
        json!({"history_ingest_requested":"true"}),
        json!({"turn_trigger":false}),
    ] {
        let body = json!({"model":"gpt-5.4","input":[user("first")],
            "client_metadata":{"x-codex-turn-metadata":turn.to_string()}});
        let result = call_core_with_headers(
            &state,
            &account,
            Bytes::from(serde_json::to_vec(&body).unwrap()),
            HeaderMap::new(),
        )
        .await;
        assert!(matches!(result, Err(CoreFailure::InvalidRequest)));
    }
    assert!(captures.lock().await.is_empty());
    let metadata = json!({"session_id":"fork-session","thread_id":"fork-child",
        "forked_from_thread_id":"fork-session","forked_from_ordinal_exclusive":7});
    let first = request(
        &state,
        &account,
        json!({"model":"gpt-5.4","input":[user("first")],"stream":false,
            "client_metadata":{"x-codex-turn-metadata":metadata.to_string()}}),
        HeaderMap::new(),
    )
    .await;
    request(
        &state,
        &account,
        json!({"model":"gpt-5.4","previous_response_id":first["id"],"input":[],"stream":false,
            "client_metadata":{"x-codex-turn-metadata":metadata.to_string()}}),
        HeaderMap::new(),
    )
    .await;
    let captures = captures.lock().await;
    assert_eq!(captures.len(), 2);
    assert_eq!(
        captures[0]["client_metadata"]["thread_id"],
        captures[1]["client_metadata"]["thread_id"]
    );
    for capture in captures.iter() {
        let turn: Value = serde_json::from_str(
            capture["client_metadata"]["x-codex-turn-metadata"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(turn["forked_from_ordinal_exclusive"], 7);
        assert_ne!(turn["forked_from_thread_id"], "fork-session");
    }
}
