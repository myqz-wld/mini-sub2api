use super::integration_support::app_state;
use super::integration_support::call_core_with_headers;
use super::*;
use crate::fingerprint::FingerprintMode;
use crate::test_support::spawn_loopback;
use crate::test_support::test_jwt;
use axum::extract::State as AxumState;
use axum::routing::post as axum_post;
use bytes::Bytes;
use http::HeaderValue;
use serde_json::Value;

#[derive(Clone, Default)]
struct CompactionCapture {
    headers: Arc<Mutex<Option<HeaderMap>>>,
    body: Arc<Mutex<Option<Bytes>>>,
}

#[tokio::test]
async fn remote_compaction_v2_uses_ordinary_responses_and_preserves_metadata() {
    let capture = CompactionCapture::default();
    let app = Router::new()
        .route(
            "/responses",
            axum_post(
                |AxumState(capture): AxumState<CompactionCapture>,
                 headers: HeaderMap,
                 body: Bytes| async move {
                    *capture.headers.lock().await = Some(headers);
                    *capture.body.lock().await = Some(body);
                    (StatusCode::OK, "captured")
                },
            ),
        )
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let account_id = "chatgpt-compaction-test";
    let metadata = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: test_jwt(Some(account_id), 7200),
                access_token: test_jwt(None, 7200),
                refresh_token: "refresh-compaction-test".to_string(),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(2)),
                issuer: upstream.base_url.clone(),
                client_id: "client-compaction-test".to_string(),
            },
            format!("{}/responses", upstream.base_url),
            FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let expected_device = vault
        .fingerprint_snapshot(&metadata.account_ref)
        .await
        .expect("fingerprint")
        .installation_id()
        .to_string();
    let state = app_state(vault);
    let turn_metadata = serde_json::json!({
        "installation_id": "compaction-conflict",
        "session_id": "session-kept",
        "thread_id": "thread-kept",
        "turn_id": "turn-kept",
        "window_id": "window-kept",
        "request_kind": "compaction",
        "compaction": {
            "trigger": "manual",
            "reason": "user_requested",
            "implementation": "responses_compaction_v2",
            "phase": "standalone_turn",
            "strategy": "memento"
        },
        "future_compaction_field": {"kept": true}
    });
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "gpt-5.6-sol",
            "instructions": "compact",
            "input": [
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "history"}]},
                {"type": "compaction_trigger"}
            ],
            "tools": [],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "reasoning": {"effort": "low"},
            "store": false,
            "stream": true,
            "include": ["reasoning.encrypted_content"],
            "prompt_cache_key": "session-kept",
            "text": {"verbosity": "low"},
            "client_metadata": {
                "x-codex-installation-id": "client-conflict",
                "x-codex-turn-metadata": turn_metadata.to_string(),
                "custom": "kept"
            }
        }))
        .expect("compaction request"),
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-codex-installation-id",
        HeaderValue::from_static("header-conflict"),
    );
    headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_str(&turn_metadata.to_string()).expect("turn metadata header"),
    );

    let response = call_core_with_headers(&state, &metadata.account_ref, body, headers)
        .await
        .expect("ordinary Responses compaction request");
    assert_eq!(response.status(), StatusCode::OK);
    let captured_headers = capture.headers.lock().await;
    let captured_headers = captured_headers.as_ref().expect("captured headers");
    assert!(!captured_headers.contains_key("x-codex-installation-id"));
    assert_eq!(
        header_text(captured_headers, "x-codex-routing-hint").as_deref(),
        Some("model=gpt-5.6-sol")
    );
    assert_eq!(
        header_text(captured_headers, "content-encoding").as_deref(),
        Some("zstd")
    );
    assert_compaction_metadata(
        &header_text(captured_headers, "x-codex-turn-metadata").expect("captured turn metadata"),
        &expected_device,
    );

    let captured_body = capture.body.lock().await;
    let body = zstd::stream::decode_all(std::io::Cursor::new(
        captured_body.as_ref().expect("captured body").as_ref(),
    ))
    .expect("decompress captured body");
    let body: Value = serde_json::from_slice(&body).expect("captured body JSON");
    assert_eq!(
        body["input"]
            .as_array()
            .and_then(|input| input.last())
            .and_then(|item| item.get("type")),
        Some(&Value::String("compaction_trigger".to_string()))
    );
    assert_eq!(body["client_metadata"]["custom"], "kept");
    assert!(
        body["client_metadata"]["x-codex-installation-id"].as_str()
            == Some(expected_device.as_str())
    );
    assert_compaction_metadata(
        body["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("client turn metadata"),
        &expected_device,
    );
}

fn assert_compaction_metadata(raw: &str, expected_device: &str) {
    let metadata: Value = serde_json::from_str(raw).expect("compaction metadata JSON");
    assert!(metadata["installation_id"].as_str() == Some(expected_device));
    assert_eq!(metadata["session_id"], "session-kept");
    assert_eq!(metadata["thread_id"], "thread-kept");
    assert_eq!(metadata["turn_id"], "turn-kept");
    assert_eq!(metadata["window_id"], "window-kept");
    assert_eq!(metadata["request_kind"], "compaction");
    assert_eq!(metadata["compaction"]["trigger"], "manual");
    assert_eq!(
        metadata["compaction"]["implementation"],
        "responses_compaction_v2"
    );
    assert_eq!(metadata["compaction"]["strategy"], "memento");
    assert_eq!(metadata["future_compaction_field"]["kept"], true);
}
