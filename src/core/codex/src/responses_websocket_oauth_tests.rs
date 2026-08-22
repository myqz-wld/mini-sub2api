use super::*;
use crate::test_support::test_jwt;
use crate::vault::CredentialMaterial;
use axum::Json;
use axum::routing::post;
use pretty_assertions::assert_eq;

#[derive(Clone)]
struct OAuthWebSocketState {
    old_access: String,
    new_access: String,
    new_id: String,
    handshake_calls: Arc<AtomicUsize>,
    refresh_calls: Arc<AtomicUsize>,
    headers: Arc<Mutex<Option<HeaderMap>>>,
    frame: Arc<Mutex<Option<String>>>,
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
        frame: Arc::new(Mutex::new(None)),
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
        )
        .await
        .expect("OAuth record");
    let core = spawn_internal(app_state(vault)).await;

    let handshake = internal_handshake(&core.base_url, &metadata.account_ref)
        .header(
            http::header::USER_AGENT,
            "codex_exec/9.9.9 (Mac OS test; arm64)",
        )
        .header("openai-organization", "must-not-cross")
        .header("x-stainless-lang", "must-not-cross")
        .upgrade()
        .send()
        .await
        .expect("refreshed handshake");
    assert_eq!(handshake.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = handshake.into_websocket().await.expect("internal socket");
    socket
        .send(UpstreamMessage::Text(
            serde_json::json!({
                "type": "response.create",
                "model": "gpt-5.6-sol",
                "input": "hello",
                "tools": [],
                "generate": false,
                "previous_response_id": "resp_previous",
                "client_metadata": {"custom": "kept"},
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
    assert!(matches!(completion, UpstreamMessage::Text(_)));
    let _ = socket.close(CloseCode::Normal, None).await;

    assert_eq!(state.handshake_calls.load(Ordering::SeqCst), 2);
    assert_eq!(state.refresh_calls.load(Ordering::SeqCst), 1);
    let frame = state.frame.lock().await.clone().expect("captured frame");
    let value: Value = serde_json::from_str(&frame).expect("normalized JSON");
    assert_eq!(value["type"], "response.create");
    assert_eq!(value["generate"], false);
    assert_eq!(value["previous_response_id"], "resp_previous");
    assert_eq!(value["client_metadata"]["custom"], "kept");
    assert_eq!(value["input"][0]["type"], "additional_tools");
    assert!(value.get("max_output_tokens").is_none());

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
        Some("codex_exec/0.147.0 (Mac OS test; arm64)")
    );
    assert_eq!(
        header_text(&headers, "openai-beta").as_deref(),
        Some(crate::upstream_request::RESPONSES_WEBSOCKET_BETA)
    );
    for forbidden in [
        "openai-organization",
        "x-stainless-lang",
        "sec-websocket-extensions",
    ] {
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
    upgrade
        .on_upgrade(move |mut socket| async move {
            let Some(Ok(InternalMessage::Text(frame))) = socket.next().await else {
                return;
            };
            *capture.frame.lock().await = Some(frame.to_string());
            let _ = socket
                .send(InternalMessage::Text(
                    r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#
                        .into(),
                ))
                .await;
        })
        .into_response()
}
