use super::*;
use crate::test_support::test_jwt;
use crate::vault::CredentialMaterial;
use axum::Json;
use axum::routing::post;
use pretty_assertions::assert_eq;
use reqwest_websocket::CloseCode as DownstreamCloseCode;
use reqwest_websocket::Message as DownstreamMessage;
use std::fs;
use std::time::Duration;
use tokio::sync::Notify;

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

#[derive(Clone, Copy)]
enum HoldingProviderEvent {
    None,
    Immediate,
    AfterRelease,
}

#[derive(Clone)]
struct HoldingOAuthWebSocketState {
    event: HoldingProviderEvent,
    handshake_calls: Arc<AtomicUsize>,
    frames: Arc<Mutex<Vec<String>>>,
    create_seen: Arc<Notify>,
    release_event: Arc<Notify>,
}

struct HoldingOAuthFixture {
    _temp: tempfile::TempDir,
    _upstream: crate::test_support::LoopbackServer,
    state: HoldingOAuthWebSocketState,
    vault: Vault,
    account_ref: String,
    account_id: String,
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
    let installation_id = "downstream-installation-native";
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
    let downstream_previous_response_id = match completion {
        DownstreamMessage::Text(text) => {
            let event: Value = serde_json::from_str(&text).expect("completion JSON");
            event["response"]["id"]
                .as_str()
                .expect("response alias")
                .to_string()
        }
        other => panic!("expected text completion, got {other:?}"),
    };
    assert_ne!(downstream_previous_response_id, "resp_first");
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
    let projected_installation = metadata["x-codex-installation-id"]
        .as_str()
        .expect("projected installation");
    assert_ne!(projected_installation, installation_id);
    assert_eq!(
        uuid::Uuid::parse_str(projected_installation)
            .expect("installation UUID")
            .get_version_num(),
        4
    );
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
    assert_eq!(projected_turn["installation_id"], projected_installation);
    assert_eq!(projected_turn["sandbox_mode"], "workspace-write");
    assert_eq!(
        projected_turn["sandbox"],
        match std::env::consts::OS {
            "macos" => "seatbelt",
            "linux" | "android" => "seccomp",
            "windows" => "windows_sandbox",
            _ => "none",
        }
    );
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
    let downstream_previous_response_id = match completion {
        DownstreamMessage::Text(text) => {
            let event: Value = serde_json::from_str(&text).expect("completion JSON");
            event["response"]["id"]
                .as_str()
                .expect("response alias")
                .to_string()
        }
        other => panic!("expected text completion, got {other:?}"),
    };
    assert_ne!(downstream_previous_response_id, "resp_first");

    socket
        .send(DownstreamMessage::Text(
            serde_json::json!({
                "type": "response.create",
                "model": "gpt-5.6-sol",
                "previous_response_id": downstream_previous_response_id,
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
    let expected_device = value["client_metadata"]["x-codex-installation-id"]
        .as_str()
        .expect("installation")
        .to_string();
    assert_eq!(
        uuid::Uuid::parse_str(&expected_device)
            .expect("installation UUID")
            .get_version_num(),
        4
    );
    assert_eq!(value["type"], "response.create");
    assert_eq!(value["generate"], false);
    assert!(value.get("previous_response_id").is_none());
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
        7
    );
    assert_eq!(turn_metadata["request_kind"], "prewarm");
    assert_eq!(turn_metadata["agent_name"], "/root");
    let turn_id = turn_metadata["turn_id"].as_str().expect("turn id");
    assert_eq!(turn_id, "");
    assert!(turn_metadata.get("root_turn_id").is_none());
    assert!(turn_metadata.get("turn_started_at_unix_ms").is_none());
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

#[tokio::test]
async fn established_subscription_socket_reports_retryable_state_unavailable_failure() {
    let account_id = "chatgpt-websocket-state-unavailable";
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
        .route("/responses", get(oauth_upstream_unbounded))
        .with_state(state.clone());
    let upstream = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: state.new_id.clone(),
                access_token: state.new_access.clone(),
                refresh_token: "refresh-state-unavailable".to_string(),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                issuer: upstream.base_url.clone(),
                client_id: "client-state-unavailable".to_string(),
            },
            format!("{}/responses", upstream.base_url),
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let state_path = vault.request_state().state_path_for_test(account_id);
    let core = spawn_internal(app_state(vault)).await;
    let handshake = internal_handshake(&core.base_url, &metadata.account_ref)
        .upgrade()
        .send()
        .await
        .expect("handshake");
    let mut socket = handshake.into_websocket().await.expect("socket");
    let create = |turn: &str| {
        serde_json::json!({
            "type":"response.create",
            "model":"gpt-5.4",
            "input":[{"type":"message","id":format!("msg_{turn}"),"role":"user","content":turn}],
            "client_metadata":{"session_id":"state-outage-session","turn_id":turn}
        })
        .to_string()
    };
    socket
        .send(DownstreamMessage::Text(create("turn-one")))
        .await
        .expect("first create");
    let first = socket
        .next()
        .await
        .expect("first completion")
        .expect("valid completion");
    assert!(matches!(first, DownstreamMessage::Text(_)));
    let frames_before_state_outage = state.frames.lock().await.len();
    fs::write(&state_path, b"{corrupt").expect("corrupt request state");

    socket
        .send(DownstreamMessage::Text(create("turn-two")))
        .await
        .expect("second create");
    let close = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("state outage close timeout")
        .expect("state outage close")
        .expect("valid state outage close");
    let DownstreamMessage::Close { code, reason } = close else {
        panic!("expected state outage close, got {close:?}");
    };
    assert_eq!(
        u16::from(code),
        mini_sub2api_protocol_v1::FAILURE_CLOSE_CODE
    );
    let failure: mini_sub2api_protocol_v1::FailureMetadata =
        serde_json::from_str(&reason).expect("state outage failure metadata");
    assert_eq!(
        failure,
        crate::error::failure(
            mini_sub2api_protocol_v1::RetryAdvice::Safe,
            mini_sub2api_protocol_v1::FailurePhase::Internal,
            mini_sub2api_protocol_v1::DeliveryState::NotDelivered,
        )
    );
    assert_eq!(
        state.frames.lock().await.len(),
        frames_before_state_outage,
        "the state-unavailable create must not reach upstream"
    );
}

#[tokio::test]
async fn first_subscription_create_reports_state_unavailable_before_provider_connect() {
    let fixture = holding_oauth_fixture(
        "chatgpt-websocket-first-state-unavailable",
        HoldingProviderEvent::None,
    )
    .await;
    fixture
        .vault
        .request_state()
        .edit(
            &fixture.account_id,
            &fixture.account_ref,
            "preseed-first-state-unavailable",
            |editor| {
                editor
                    .installation_id(crate::fingerprint::FingerprintMode::Device, None)
                    .map(|_| ())
            },
        )
        .await
        .expect("preseed request state");
    let state_path = fixture
        .vault
        .request_state()
        .state_path_for_test(&fixture.account_id);
    fs::write(state_path, b"{corrupt").expect("corrupt request state");

    let core = spawn_internal(app_state(fixture.vault.clone())).await;
    let handshake = internal_handshake(&core.base_url, &fixture.account_ref)
        .upgrade()
        .send()
        .await
        .expect("handshake");
    let mut socket = handshake.into_websocket().await.expect("socket");
    socket
        .send(DownstreamMessage::Text(stateful_create("first")))
        .await
        .expect("first create");
    let close = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("first-create failure timeout")
        .expect("first-create failure")
        .expect("valid first-create failure");
    assert_eq!(
        failure_metadata(close),
        crate::error::failure(
            mini_sub2api_protocol_v1::RetryAdvice::Safe,
            mini_sub2api_protocol_v1::FailurePhase::Internal,
            mini_sub2api_protocol_v1::DeliveryState::NotDelivered,
        )
    );
    assert_eq!(fixture.state.handshake_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.state.frames.lock().await.is_empty());
}

#[tokio::test]
async fn missing_control_reference_preserves_attempted_delivery() {
    let fixture = holding_oauth_fixture(
        "chatgpt-websocket-attempted-control-state-unavailable",
        HoldingProviderEvent::None,
    )
    .await;
    let core = spawn_internal(app_state(fixture.vault.clone())).await;
    let handshake = internal_handshake(&core.base_url, &fixture.account_ref)
        .upgrade()
        .send()
        .await
        .expect("handshake");
    let mut socket = handshake.into_websocket().await.expect("socket");
    let create_seen = fixture.state.create_seen.notified();
    socket
        .send(DownstreamMessage::Text(stateful_create("attempted")))
        .await
        .expect("create");
    tokio::time::timeout(Duration::from_secs(2), create_seen)
        .await
        .expect("provider did not receive create");
    socket
        .send(DownstreamMessage::Text(stateful_inject(
            "resp_not_observed",
        )))
        .await
        .expect("inject");
    let close = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("attempted failure timeout")
        .expect("attempted failure")
        .expect("valid attempted failure");
    assert_eq!(
        failure_metadata(close),
        crate::error::failure(
            mini_sub2api_protocol_v1::RetryAdvice::Ambiguous,
            mini_sub2api_protocol_v1::FailurePhase::Internal,
            mini_sub2api_protocol_v1::DeliveryState::PossiblyDelivered,
        )
    );
    assert_eq!(fixture.state.frames.lock().await.len(), 1);
}

#[tokio::test]
async fn state_failure_on_control_frame_preserves_observed_delivery() {
    let fixture = holding_oauth_fixture(
        "chatgpt-websocket-observed-control-state-unavailable",
        HoldingProviderEvent::Immediate,
    )
    .await;
    let state_path = fixture
        .vault
        .request_state()
        .state_path_for_test(&fixture.account_id);
    let core = spawn_internal(app_state(fixture.vault.clone())).await;
    let handshake = internal_handshake(&core.base_url, &fixture.account_ref)
        .upgrade()
        .send()
        .await
        .expect("handshake");
    let mut socket = handshake.into_websocket().await.expect("socket");
    socket
        .send(DownstreamMessage::Text(stateful_create("observed")))
        .await
        .expect("create");
    let event = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("provider event timeout")
        .expect("provider event")
        .expect("valid provider event");
    let DownstreamMessage::Text(event) = event else {
        panic!("expected provider event, got {event:?}");
    };
    let event: Value = serde_json::from_str(&event).expect("provider event JSON");
    let response_id = event["response"]["id"]
        .as_str()
        .expect("downstream response id")
        .to_string();
    fs::write(&state_path, b"{corrupt").expect("corrupt request state");

    socket
        .send(DownstreamMessage::Text(stateful_inject(&response_id)))
        .await
        .expect("inject");
    let close = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("observed failure timeout")
        .expect("observed failure")
        .expect("valid observed failure");
    assert_eq!(
        failure_metadata(close),
        crate::error::failure(
            mini_sub2api_protocol_v1::RetryAdvice::Never,
            mini_sub2api_protocol_v1::FailurePhase::Internal,
            mini_sub2api_protocol_v1::DeliveryState::Delivered,
        )
    );
    assert_eq!(fixture.state.frames.lock().await.len(), 1);
}

#[tokio::test]
async fn websocket_translation_failure_after_provider_event_is_delivered() {
    let fixture = holding_oauth_fixture(
        "chatgpt-websocket-response-translation-state-unavailable",
        HoldingProviderEvent::AfterRelease,
    )
    .await;
    let state_path = fixture
        .vault
        .request_state()
        .state_path_for_test(&fixture.account_id);
    let core = spawn_internal(app_state(fixture.vault.clone())).await;
    let handshake = internal_handshake(&core.base_url, &fixture.account_ref)
        .upgrade()
        .send()
        .await
        .expect("handshake");
    let mut socket = handshake.into_websocket().await.expect("socket");
    let create_seen = fixture.state.create_seen.notified();
    socket
        .send(DownstreamMessage::Text(stateful_create("translate")))
        .await
        .expect("create");
    tokio::time::timeout(Duration::from_secs(2), create_seen)
        .await
        .expect("provider did not receive create");
    fs::write(&state_path, b"{corrupt").expect("corrupt request state");
    fixture.state.release_event.notify_one();

    let close = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("translation failure timeout")
        .expect("translation failure")
        .expect("valid translation failure");
    assert_eq!(
        failure_metadata(close),
        crate::error::failure(
            mini_sub2api_protocol_v1::RetryAdvice::Never,
            mini_sub2api_protocol_v1::FailurePhase::WebSocketRelay,
            mini_sub2api_protocol_v1::DeliveryState::Delivered,
        )
    );
    assert_eq!(fixture.state.frames.lock().await.len(), 1);
}

async fn holding_oauth_fixture(
    account_id: &str,
    event: HoldingProviderEvent,
) -> HoldingOAuthFixture {
    let state = HoldingOAuthWebSocketState {
        event,
        handshake_calls: Arc::new(AtomicUsize::new(0)),
        frames: Arc::new(Mutex::new(Vec::new())),
        create_seen: Arc::new(Notify::new()),
        release_event: Arc::new(Notify::new()),
    };
    let app = Router::new()
        .route("/responses", get(holding_oauth_upstream))
        .with_state(state.clone());
    let upstream = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: test_jwt(Some(account_id), 7200),
                access_token: test_jwt(None, 7200),
                refresh_token: format!("refresh-{account_id}"),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                issuer: upstream.base_url.clone(),
                client_id: format!("client-{account_id}"),
            },
            format!("{}/responses", upstream.base_url),
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    HoldingOAuthFixture {
        _temp: temp,
        _upstream: upstream,
        state,
        vault,
        account_ref: metadata.account_ref,
        account_id: account_id.to_string(),
    }
}

fn stateful_create(turn: &str) -> String {
    serde_json::json!({
        "type":"response.create",
        "model":"gpt-5.4",
        "generate":true,
        "input":[{"type":"message","id":format!("msg_{turn}"),"role":"user","content":turn}],
        "client_metadata":{"session_id":"state-outage-session","turn_id":turn}
    })
    .to_string()
}

fn stateful_inject(response_id: &str) -> String {
    serde_json::json!({
        "type":"response.inject",
        "response_id":response_id,
        "input":[{
            "type":"function_call_output",
            "id":"fco_state_outage",
            "call_id":"call_state_outage",
            "output":"ok"
        }]
    })
    .to_string()
}

fn failure_metadata(message: DownstreamMessage) -> mini_sub2api_protocol_v1::FailureMetadata {
    let DownstreamMessage::Close { code, reason } = message else {
        panic!("expected failure close, got {message:?}");
    };
    assert_eq!(
        u16::from(code),
        mini_sub2api_protocol_v1::FAILURE_CLOSE_CODE
    );
    serde_json::from_str(&reason).expect("failure metadata")
}

async fn holding_oauth_upstream(
    AxumState(state): AxumState<HoldingOAuthWebSocketState>,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    state.handshake_calls.fetch_add(1, Ordering::SeqCst);
    let capture = state.clone();
    upgrade
        .on_upgrade(move |mut socket| async move {
            let Some(Ok(InternalMessage::Text(create))) = socket.next().await else {
                return;
            };
            capture.frames.lock().await.push(create.to_string());
            capture.create_seen.notify_one();
            match capture.event {
                HoldingProviderEvent::None => {}
                HoldingProviderEvent::Immediate => {}
                HoldingProviderEvent::AfterRelease => capture.release_event.notified().await,
            }
            if !matches!(capture.event, HoldingProviderEvent::None) {
                let event = serde_json::json!({
                    "type":"response.created",
                    "response":{"id":"resp_holding"}
                });
                if socket
                    .send(InternalMessage::Text(event.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
                capture.frames.lock().await.push(frame.to_string());
            }
        })
        .into_response()
}

#[path = "responses_websocket_oauth_reference_tests.rs"]
mod reference_tests;

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
            for index in 0..2 {
                let Some(Ok(InternalMessage::Text(frame))) = socket.next().await else {
                    return;
                };
                capture.frames.lock().await.push(frame.to_string());
                let response_id = if index == 0 {
                    "resp_first"
                } else {
                    "resp_second"
                };
                let event = serde_json::json!({
                    "type":"response.completed",
                    "response":{
                        "id":response_id,
                        "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
                    }
                });
                let _ = socket
                    .send(InternalMessage::Text(event.to_string().into()))
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

async fn oauth_upstream_unbounded(
    AxumState(state): AxumState<OAuthWebSocketState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    state.handshake_calls.fetch_add(1, Ordering::SeqCst);
    *state.headers.lock().await = Some(headers);
    let capture = state.clone();
    upgrade
        .on_upgrade(move |mut socket| async move {
            let mut index = 0_u64;
            while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
                capture.frames.lock().await.push(frame.to_string());
                let event = serde_json::json!({
                    "type":"response.completed",
                    "response":{
                        "id":format!("resp_state_{index}"),
                        "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
                    }
                });
                index = index.saturating_add(1);
                if socket
                    .send(InternalMessage::Text(event.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        })
        .into_response()
}
