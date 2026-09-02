use super::*;

const INSTALLATION_ID: &str = "11111111-1111-4111-8111-111111111111";
const ACCOUNT_NAMESPACE: &str = "chatgpt-account-native";
const PSEUDONYM_SCOPE: &str = "psn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const SESSION_ID: &str = "session-native";
const THREAD_ID: &str = "thread-native";
const WINDOW_ID: &str = "thread-native:0";

#[tokio::test]
async fn native_websocket_turn_pseudonymizes_identity_deterministically() {
    let turn_metadata = r#"{"installation_id":"11111111-1111-4111-8111-111111111111","session_id":"session-native","thread_id":"thread-native","agent_name":"/root","turn_id":"turn-native","window_id":"thread-native:0","request_kind":"turn","root_turn_id":"turn-native","auto_review_enabled":false,"node_repl_auto_review_required":false,"node_repl_disabled":false,"turn_started_at_unix_ms":1700000000000}"#;
    let encoded_turn_metadata = serde_json::to_string(turn_metadata).expect("turn metadata string");
    let body = Bytes::from(format!(
        r#"{{"type":"response.create","model":"gpt-5.4","input":[],"tools":[],"tool_choice":"auto","parallel_tool_calls":true,"reasoning":{{"effort":"medium"}},"store":false,"stream":true,"include":["reasoning.encrypted_content"],"prompt_cache_key":"session-native","text":{{"verbosity":"low"}},"client_metadata":{{"session_id":"session-native","thread_id":"thread-native","turn_id":"turn-native","x-codex-installation-id":"11111111-1111-4111-8111-111111111111","x-codex-turn-metadata":{encoded_turn_metadata},"x-codex-window-id":"thread-native:0","root_turn_id":"turn-native","x-codex-ws-stream-request-start-ms":"1700000000123"}}}}"#
    ));

    let harness = CodexStateTestHarness::new();
    let prepared = harness
        .prepare(
            UpstreamProfile::CodexSubscription149,
            EmulationTransport::WebSocket,
            &native_headers(),
            body.clone(),
            16 * 1024,
            "acct_native_ws",
            ACCOUNT_NAMESPACE,
            PSEUDONYM_SCOPE,
        )
        .await
        .expect("normalized request");

    let repeated = harness
        .prepare(
            UpstreamProfile::CodexSubscription149,
            EmulationTransport::WebSocket,
            &native_headers(),
            body,
            16 * 1024,
            "acct_native_ws",
            ACCOUNT_NAMESPACE,
            PSEUDONYM_SCOPE,
        )
        .await
        .expect("normalized request");
    assert_eq!(prepared.body, repeated.body);
    assert_eq!(prepared.headers, repeated.headers);
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized request");
    let metadata = value["client_metadata"]
        .as_object()
        .expect("client metadata");
    for (name, raw) in [
        ("session_id", SESSION_ID),
        ("thread_id", THREAD_ID),
        ("turn_id", "turn-native"),
        ("x-codex-installation-id", INSTALLATION_ID),
    ] {
        let pseudonym = metadata[name].as_str().expect("pseudonym");
        assert_ne!(pseudonym, raw);
        assert_uuid_version(
            pseudonym,
            if name == "x-codex-installation-id" {
                4
            } else {
                7
            },
        );
    }
    assert_eq!(
        metadata["x-codex-window-id"],
        format!("{}:0", metadata["thread_id"].as_str().expect("thread"))
    );
    assert_eq!(
        metadata["x-codex-ws-stream-request-start-ms"],
        "1700000000123"
    );
}

#[tokio::test]
async fn native_websocket_prewarm_keeps_empty_turn_semantics_after_pseudonymization() {
    let turn_metadata = r#"{"installation_id":"11111111-1111-4111-8111-111111111111","session_id":"session-native","thread_id":"thread-native","agent_name":"/root","turn_id":"","window_id":"thread-native:0","request_kind":"prewarm","sandbox":"workspace-write","sandbox_mode":"workspace-write","auto_review_enabled":false,"node_repl_auto_review_required":false,"node_repl_disabled":false}"#;
    let encoded_turn_metadata = serde_json::to_string(turn_metadata).expect("turn metadata string");
    let body = Bytes::from(format!(
        r#"{{"type":"response.create","model":"gpt-5.4","input":[],"tools":[],"tool_choice":"auto","parallel_tool_calls":true,"reasoning":{{"effort":"medium"}},"store":false,"stream":true,"include":["reasoning.encrypted_content"],"prompt_cache_key":"session-native","text":{{"verbosity":"low"}},"generate":false,"client_metadata":{{"session_id":"session-native","thread_id":"thread-native","turn_id":"","x-codex-installation-id":"11111111-1111-4111-8111-111111111111","x-codex-turn-metadata":{encoded_turn_metadata},"x-codex-window-id":"thread-native:0","x-codex-ws-stream-request-start-ms":"1700000000123"}}}}"#
    ));

    let harness = CodexStateTestHarness::new();
    let prepared = harness
        .prepare(
            UpstreamProfile::CodexSubscription149,
            EmulationTransport::WebSocket,
            &native_headers(),
            body.clone(),
            16 * 1024,
            "acct_native_ws_prewarm",
            ACCOUNT_NAMESPACE,
            PSEUDONYM_SCOPE,
        )
        .await
        .expect("normalized request");

    assert_ne!(prepared.body, body);
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized request");
    let metadata = &value["client_metadata"];
    assert_eq!(metadata["turn_id"], "");
    assert_ne!(metadata["session_id"], SESSION_ID);
    assert_ne!(metadata["thread_id"], THREAD_ID);
    assert_eq!(
        metadata["x-codex-ws-stream-request-start-ms"],
        "1700000000123"
    );
    let turn: Value = serde_json::from_str(
        metadata["x-codex-turn-metadata"]
            .as_str()
            .expect("turn metadata"),
    )
    .expect("turn metadata JSON");
    assert_eq!(turn["turn_id"], "");
    assert!(turn.get("root_turn_id").is_none());
    assert!(turn.get("turn_started_at_unix_ms").is_none());
    assert_eq!(turn["session_id"], metadata["session_id"]);
    assert_eq!(turn["thread_id"], metadata["thread_id"]);
    assert_eq!(
        header_text(&prepared.headers, "x-codex-turn-metadata").as_deref(),
        metadata["x-codex-turn-metadata"].as_str()
    );
}

#[test]
fn incomplete_websocket_prewarm_still_gets_missing_metadata() {
    let prepared = prepare_prewarm("{\"request_kind\":\"prewarm\",\"turn_id\":\"\"}", "");
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized request");
    let client_metadata = value["client_metadata"]
        .as_object()
        .expect("client metadata");
    assert!(client_metadata.get("root_turn_id").is_none());
    assert!(
        client_metadata["x-codex-ws-stream-request-start-ms"]
            .as_str()
            .and_then(|value| value.parse::<i64>().ok())
            .is_some_and(|value| value > 0)
    );
    let turn_metadata: Value = serde_json::from_str(
        client_metadata["x-codex-turn-metadata"]
            .as_str()
            .expect("turn metadata"),
    )
    .expect("turn metadata JSON");
    assert_eq!(turn_metadata["request_kind"], "prewarm");
    assert_eq!(turn_metadata["turn_id"], "");
    assert!(turn_metadata.get("root_turn_id").is_none());
    assert!(
        turn_metadata["turn_started_at_unix_ms"]
            .as_i64()
            .is_some_and(|value| value > 0)
    );
}

#[test]
fn invalid_native_prewarm_shape_is_not_treated_as_complete() {
    let turn_metadata = r#"{"installation_id":"11111111-1111-4111-8111-111111111111","session_id":"session-native","thread_id":"thread-native","agent_name":null,"turn_id":"","window_id":"thread-native:0","request_kind":"prewarm","sandbox":"workspace-write","sandbox_mode":"workspace-write","auto_review_enabled":false,"node_repl_auto_review_required":false,"node_repl_disabled":false}"#;
    let prepared = prepare_prewarm(turn_metadata, "1700000000123");
    let value: Value = serde_json::from_slice(&prepared.body).expect("normalized request");
    let normalized: Value = serde_json::from_str(
        value["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("turn metadata"),
    )
    .expect("turn metadata JSON");

    assert!(
        normalized["turn_started_at_unix_ms"]
            .as_i64()
            .is_some_and(|value| value > 0)
    );
}

fn prepare_prewarm(turn_metadata: &str, stream_start_ms: &str) -> PreparedEmulatedRequest {
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "type": "response.create",
            "model": "gpt-5.4",
            "input": [],
            "tools": [],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "reasoning": {"effort": "medium"},
            "store": false,
            "stream": true,
            "include": ["reasoning.encrypted_content"],
            "prompt_cache_key": SESSION_ID,
            "text": {"verbosity": "low"},
            "generate": false,
            "client_metadata": {
                "session_id": SESSION_ID,
                "thread_id": THREAD_ID,
                "turn_id": "",
                "x-codex-installation-id": INSTALLATION_ID,
                "x-codex-turn-metadata": turn_metadata,
                "x-codex-window-id": WINDOW_ID,
                "x-codex-ws-stream-request-start-ms": stream_start_ms
            }
        }))
        .expect("prewarm request"),
    );
    prepare_codex_overlay_for_test(
        UpstreamProfile::CodexSubscription149,
        EmulationTransport::WebSocket,
        &native_headers(),
        body,
        16 * 1024,
    )
    .expect("normalized prewarm request")
}

fn native_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("session-id", SESSION_ID),
        ("thread-id", THREAD_ID),
        ("x-client-request-id", THREAD_ID),
        ("x-codex-window-id", WINDOW_ID),
    ] {
        headers.insert(name, value.parse().expect("header"));
    }
    headers
}

fn assert_uuid_version(value: &str, version: usize) {
    assert_eq!(
        uuid::Uuid::parse_str(value)
            .expect("pseudonym UUID")
            .get_version_num(),
        version
    );
}
