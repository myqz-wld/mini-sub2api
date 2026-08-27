use super::*;
use crate::test_support::test_jwt;
use crate::vault::CredentialMaterial;
use axum::Json;
use axum::routing::post;
use pretty_assertions::assert_eq;
use reqwest_websocket::CloseCode as DownstreamCloseCode;
use reqwest_websocket::Message as DownstreamMessage;

#[derive(Clone)]
struct OAuthWebSocketState {
    old_access: String,
    new_access: String,
    new_id: String,
    handshake_calls: Arc<AtomicUsize>,
    refresh_calls: Arc<AtomicUsize>,
    headers: Arc<Mutex<Option<HeaderMap>>>,
    frames: Arc<Mutex<Vec<String>>>,
}

#[tokio::test]
async fn oauth_pseudonymizes_native_prewarm_identity_and_preserves_semantics() {
    let account_id = "chatgpt-native-prewarm-test";
    let state = OAuthWebSocketState {
        old_access: test_jwt(None, 3600),
        new_access: test_jwt(None, 7200),
        new_id: test_jwt(Some(account_id), 7200),
        handshake_calls: Arc::new(AtomicUsize::new(0)),
        refresh_calls: Arc::new(AtomicUsize::new(0)),
        headers: Arc::new(Mutex::new(None)),
        frames: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/responses", get(oauth_upstream))
        .with_state(state.clone());
    let upstream = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: state.new_id.clone(),
                access_token: state.new_access.clone(),
                refresh_token: "refresh-native-prewarm-test".to_string(),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                issuer: upstream.base_url.clone(),
                client_id: "client-native-prewarm-test".to_string(),
            },
            format!("{}/responses", upstream.base_url),
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let installation_id =
        crate::request_pseudonym::RequestPseudonymizer::converged_installation_id(account_id);
    let core = spawn_internal(app_state(vault)).await;
    let handshake = internal_handshake(&core.base_url, &metadata.account_ref)
        .upgrade()
        .send()
        .await
        .expect("handshake");
    let mut socket = handshake.into_websocket().await.expect("socket");
    let turn_metadata = format!(
        r#"{{"installation_id":"{installation_id}","session_id":"session-native","thread_id":"thread-native","agent_name":"/root","turn_id":"","window_id":"thread-native:0","request_kind":"prewarm","sandbox":"workspace-write","sandbox_mode":"workspace-write","auto_review_enabled":false,"node_repl_auto_review_required":false,"node_repl_disabled":false}}"#
    );
    let encoded_turn_metadata =
        serde_json::to_string(&turn_metadata).expect("turn metadata string");
    let frame = format!(
        r#"{{"type":"response.create","model":"gpt-5.4","input":[],"tools":[],"tool_choice":"auto","parallel_tool_calls":true,"reasoning":{{"effort":"medium"}},"store":false,"stream":true,"include":["reasoning.encrypted_content"],"prompt_cache_key":"session-native","text":{{"verbosity":"low"}},"generate":false,"client_metadata":{{"session_id":"session-native","thread_id":"thread-native","turn_id":"","x-codex-installation-id":"{installation_id}","x-codex-turn-metadata":{encoded_turn_metadata},"x-codex-window-id":"thread-native:0","x-codex-ws-stream-request-start-ms":"1700000000123"}}}}"#
    );
    socket
        .send(DownstreamMessage::Text(frame.clone()))
        .await
        .expect("send native prewarm");
    let completion = socket
        .next()
        .await
        .expect("completion")
        .expect("valid event");
    assert!(matches!(completion, DownstreamMessage::Text(_)));
    let _ = socket.close(DownstreamCloseCode::Normal, None).await;

    assert_eq!(state.handshake_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.refresh_calls.load(Ordering::SeqCst), 0);
    let frames = state.frames.lock().await;
    assert_eq!(frames.len(), 1);
    assert_ne!(frames[0], frame);
    let projected: Value = serde_json::from_str(&frames[0]).expect("projected frame");
    let metadata = &projected["client_metadata"];
    assert_ne!(metadata["session_id"], "session-native");
    assert_ne!(metadata["thread_id"], "thread-native");
    assert_eq!(metadata["turn_id"], "");
    assert_eq!(metadata["x-codex-installation-id"], installation_id);
    assert_eq!(
        metadata["x-codex-ws-stream-request-start-ms"],
        "1700000000123"
    );
    let projected_turn: Value = serde_json::from_str(
        metadata["x-codex-turn-metadata"]
            .as_str()
            .expect("projected turn metadata"),
    )
    .expect("projected turn metadata JSON");
    assert_eq!(projected_turn["session_id"], metadata["session_id"]);
    assert_eq!(projected_turn["thread_id"], metadata["thread_id"]);
    assert_eq!(projected_turn["turn_id"], "");
    drop(frames);
    let headers = state.headers.lock().await;
    assert_eq!(
        header_text(
            headers.as_ref().expect("captured headers"),
            "x-codex-turn-metadata"
        )
        .as_deref(),
        metadata["x-codex-turn-metadata"].as_str()
    );
}

#[tokio::test]
async fn oauth_handshake_401_refreshes_once_then_normalizes_create_frame() {
    let account_id = "chatgpt-websocket-test";
    let state = OAuthWebSocketState {
        old_access: test_jwt(None, 3600),
        new_access: test_jwt(None, 7200),
        new_id: test_jwt(Some(account_id), 7200),
        handshake_calls: Arc::new(AtomicUsize::new(0)),
        refresh_calls: Arc::new(AtomicUsize::new(0)),
        headers: Arc::new(Mutex::new(None)),
        frames: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/responses", get(oauth_upstream))
        .route(
            "/oauth/token",
            post(
                |AxumState(state): AxumState<OAuthWebSocketState>| async move {
                    state.refresh_calls.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({
                        "id_token": state.new_id,
                        "access_token": state.new_access,
                        "refresh_token": "refresh-new-websocket-test"
                    }))
                },
            ),
        )
        .with_state(state.clone());
    let upstream = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: test_jwt(Some(account_id), 3600),
                access_token: state.old_access.clone(),
                refresh_token: "refresh-old-websocket-test".to_string(),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                issuer: upstream.base_url.clone(),
                client_id: "client-websocket-test".to_string(),
            },
            format!("{}/responses", upstream.base_url),
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let expected_device =
        crate::request_pseudonym::RequestPseudonymizer::converged_installation_id(account_id);
    let vault_after_refresh = vault.clone();
    let core = spawn_internal(app_state(vault)).await;

    let handshake = internal_handshake(&core.base_url, &metadata.account_ref)
        .header(
            http::header::USER_AGENT,
            "codex_exec/9.9.9 (Mac OS test; arm64)",
        )
        .header(crate::upstream_request::CODEX_VERSION_HEADER, "9.9.9")
        .header("x-openai-subagent", "review")
        .header("openai-organization", "must-not-cross")
        .header("x-stainless-lang", "must-not-cross")
        .header("x-codex-installation-id", "handshake-conflict")
        .header(
            "x-codex-turn-metadata",
            r#"{"installation_id":"turn-conflict","session_id":"session-kept"}"#,
        )
        .upgrade()
        .send()
        .await
        .expect("refreshed handshake");
    assert_eq!(handshake.status(), StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(state.handshake_calls.load(Ordering::SeqCst), 0);
    let mut socket = handshake.into_websocket().await.expect("internal socket");
    socket
        .send(DownstreamMessage::Text(
            serde_json::json!({
                "type": "response.create",
                "model": "gpt-5.6-sol",
                "service_tier": "priority",
                "input": "hello",
                "tools": [],
                "generate": false,
                "previous_response_id": "resp_previous",
                "client_metadata": {
                    "custom": "kept",
                    "x-codex-installation-id": "frame-conflict",
                    "x-codex-turn-metadata": "{\"installation_id\":\"frame-turn-conflict\",\"thread_id\":\"thread-kept\"}"
                },
                "max_output_tokens": 1000
            })
            .to_string(),
        ))
        .await
        .expect("send create frame");
    let completion = socket
        .next()
        .await
        .expect("completion")
        .expect("valid event");
    assert!(matches!(completion, DownstreamMessage::Text(_)));

    socket
        .send(DownstreamMessage::Text(
            serde_json::json!({
                "type": "response.create",
                "model": "gpt-5.6-sol",
                "previous_response_id": "resp_first",
                "input": [],
                "tool_choice": "auto",
                "parallel_tool_calls": false,
                "reasoning": {"effort": "low", "context": "all_turns"},
                "store": false,
                "stream": true,
                "include": ["reasoning.encrypted_content"],
                "prompt_cache_key": "session-kept",
                "text": {"verbosity": "low"},
                "client_metadata": {
                    "session_id": "session-kept",
                    "thread_id": "thread-kept",
                    "turn_id": "turn-two"
                }
            })
            .to_string(),
        ))
        .await
        .expect("send reused create frame");
    let second_completion = socket
        .next()
        .await
        .expect("second completion")
        .expect("valid second event");
    assert!(matches!(second_completion, DownstreamMessage::Text(_)));
    let _ = socket.close(DownstreamCloseCode::Normal, None).await;

    assert_eq!(state.handshake_calls.load(Ordering::SeqCst), 2);
    assert_eq!(state.refresh_calls.load(Ordering::SeqCst), 1);
    let frames = state.frames.lock().await.clone();
    assert_eq!(frames.len(), 2);
    let frame = &frames[0];
    let value: Value = serde_json::from_str(frame).expect("normalized JSON");
    assert_eq!(value["type"], "response.create");
    assert_eq!(value["generate"], false);
    assert_eq!(value["previous_response_id"], "resp_previous");
    assert_eq!(value["client_metadata"]["custom"], "kept");
    assert_eq!(
        value["client_metadata"]["ws_request_header_x_openai_internal_codex_responses_lite"],
        "true"
    );
    assert!(
        value["client_metadata"]["x-codex-ws-stream-request-start-ms"]
            .as_str()
            .and_then(|value| value.parse::<i64>().ok())
            .is_some_and(|value| value > 0)
    );
    assert!(
        value["client_metadata"]["x-codex-installation-id"].as_str()
            == Some(expected_device.as_str())
    );
    let turn_metadata: Value = serde_json::from_str(
        value["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("turn metadata"),
    )
    .expect("turn metadata JSON");
    assert!(turn_metadata["installation_id"].as_str() == Some(expected_device.as_str()));
    let thread_id = turn_metadata["thread_id"].as_str().expect("thread id");
    assert_ne!(thread_id, "thread-kept");
    assert_eq!(
        uuid::Uuid::parse_str(thread_id)
            .expect("thread pseudonym")
            .get_version_num(),
        8
    );
    assert_eq!(turn_metadata["request_kind"], "prewarm");
    assert_eq!(turn_metadata["agent_name"], "/root");
    let turn_id = turn_metadata["turn_id"].as_str().expect("turn id");
    assert_eq!(turn_metadata["root_turn_id"], turn_id);
    assert_eq!(
        uuid::Uuid::parse_str(turn_id)
            .expect("turn UUID")
            .get_version_num(),
        7
    );
    assert!(
        turn_metadata["turn_started_at_unix_ms"]
            .as_i64()
            .is_some_and(|value| value > 0)
    );
    assert_eq!(value["input"][0]["type"], "additional_tools");
    assert!(value["input"][0].get("id").is_none());
    assert_eq!(
        value["input"][1]["content"][0]["text"],
        crate::codex_instructions::for_model("gpt-5.6-sol")
    );
    assert!(value["input"][1].get("id").is_none());
    assert!(
        value["input"][2]["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("msg_"))
    );
    assert_eq!(
        value["input"][2]["internal_chat_message_metadata_passthrough"]["turn_id"],
        turn_id
    );
    assert!(value.get("max_output_tokens").is_none());
    let reused: Value = serde_json::from_str(&frames[1]).expect("reused frame JSON");
    assert_eq!(reused["previous_response_id"], "resp_first");
    assert_eq!(reused["input"], serde_json::json!([]));
    assert_eq!(
        reused["client_metadata"]["x-codex-turn-state"],
        "provider-turn-state"
    );

    let headers = state
        .headers
        .lock()
        .await
        .clone()
        .expect("captured headers");
    assert_eq!(
        header_text(&headers, http::header::AUTHORIZATION.as_str()).as_deref(),
        Some(format!("Bearer {}", state.new_access).as_str())
    );
    assert_eq!(
        header_text(&headers, "chatgpt-account-id").as_deref(),
        Some(account_id)
    );
    assert_eq!(
        header_text(&headers, http::header::USER_AGENT.as_str()).as_deref(),
        Some(crate::codex_user_agent::canonical_value().as_str())
    );
    assert_eq!(
        header_text(&headers, "originator").as_deref(),
        Some(crate::upstream_request::DEFAULT_CODEX_ORIGINATOR)
    );
    assert_eq!(
        header_text(&headers, crate::upstream_request::CODEX_VERSION_HEADER).as_deref(),
        Some(crate::upstream_request::CODEX_COMPATIBILITY_VERSION)
    );
    assert_eq!(
        header_text(&headers, "x-openai-subagent").as_deref(),
        Some("review")
    );
    assert_eq!(
        header_text(&headers, "openai-beta").as_deref(),
        Some(crate::upstream_request::RESPONSES_WEBSOCKET_BETA)
    );
    assert_eq!(
        header_text(&headers, "sec-websocket-extensions").as_deref(),
        Some("permessage-deflate; client_max_window_bits")
    );
    assert!(!headers.contains_key("x-codex-installation-id"));
    assert_eq!(
        header_text(&headers, crate::upstream_request::CODEX_ROUTING_HINT_HEADER).as_deref(),
        Some("model=gpt-5.6-sol;tier=priority")
    );
    assert!(!headers.contains_key("x-openai-internal-codex-responses-lite"));
    assert!(!headers.contains_key(mini_sub2api_protocol_v1::PSEUDONYM_SCOPE_HEADER));
    assert_eq!(
        header_text(&headers, "x-client-request-id"),
        header_text(&headers, "thread-id")
    );
    let header_turn: Value = serde_json::from_str(
        header_text(&headers, "x-codex-turn-metadata")
            .as_deref()
            .expect("handshake turn metadata"),
    )
    .expect("handshake turn metadata JSON");
    assert!(header_turn["installation_id"].as_str() == Some(expected_device.as_str()));
    assert_eq!(header_turn, turn_metadata);
    assert_ne!(header_turn["session_id"], "session-kept");
    assert_eq!(header_turn["thread_id"], thread_id);
    assert_eq!(header_turn["request_kind"], "prewarm");
    let fingerprint_after_refresh = vault_after_refresh
        .fingerprint_snapshot(&metadata.account_ref)
        .await
        .expect("fingerprint after refresh");
    assert_eq!(fingerprint_after_refresh.mode(), FingerprintMode::Device);
    assert_eq!(fingerprint_after_refresh.revision(), 1);
    for forbidden in ["openai-organization", "x-stainless-lang"] {
        assert!(!headers.contains_key(forbidden), "header {forbidden}");
    }
}

async fn oauth_upstream(
    AxumState(state): AxumState<OAuthWebSocketState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    state.handshake_calls.fetch_add(1, Ordering::SeqCst);
    let authorization = header_text(&headers, http::header::AUTHORIZATION.as_str());
    if authorization.as_deref() == Some(format!("Bearer {}", state.old_access).as_str()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": {"code": "expired"}})),
        )
            .into_response();
    }
    *state.headers.lock().await = Some(headers);
    let capture = state.clone();
    let mut response = upgrade
        .on_upgrade(move |mut socket| async move {
            for _ in 0..2 {
                let Some(Ok(InternalMessage::Text(frame))) = socket.next().await else {
                    return;
                };
                capture.frames.lock().await.push(frame.to_string());
                let _ = socket
                    .send(InternalMessage::Text(
                        r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#
                            .into(),
                    ))
                    .await;
            }
        })
        .into_response();
    response.headers_mut().insert(
        "x-codex-turn-state",
        HeaderValue::from_static("provider-turn-state"),
    );
    response
}
