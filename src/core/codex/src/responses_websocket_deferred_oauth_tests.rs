use super::*;
use crate::test_support::test_jwt;
use crate::vault::CredentialMaterial;
use axum::Json;
use axum::routing::post;
use pretty_assertions::assert_eq;

#[derive(Clone)]
struct ReconnectOAuthState {
    old_access: String,
    new_access: String,
    new_id: String,
    authorizations: Arc<Mutex<Vec<String>>>,
    accepted_headers: Arc<Mutex<Vec<HeaderMap>>>,
    accepted: Arc<AtomicUsize>,
    refreshes: Arc<AtomicUsize>,
    frames: Arc<Mutex<Vec<String>>>,
}

#[tokio::test]
async fn refreshed_oauth_is_reused_for_hidden_setup_reconnect() {
    let account_id = "chatgpt-hidden-reconnect-test";
    let state = ReconnectOAuthState {
        old_access: test_jwt(None, 3600),
        new_access: test_jwt(None, 7200),
        new_id: test_jwt(Some(account_id), 7200),
        authorizations: Arc::new(Mutex::new(Vec::new())),
        accepted_headers: Arc::new(Mutex::new(Vec::new())),
        accepted: Arc::new(AtomicUsize::new(0)),
        refreshes: Arc::new(AtomicUsize::new(0)),
        frames: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/responses", get(reconnect_oauth_upstream))
        .route(
            "/oauth/token",
            post(
                |AxumState(state): AxumState<ReconnectOAuthState>| async move {
                    state.refreshes.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({
                        "id_token": state.new_id,
                        "access_token": state.new_access,
                        "refresh_token": "refresh-hidden-reconnect-new"
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
                refresh_token: "refresh-hidden-reconnect-old".to_string(),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                issuer: upstream.base_url.clone(),
                client_id: "client-hidden-reconnect".to_string(),
            },
            format!("{}/responses", upstream.base_url),
            FingerprintMode::Off,
        )
        .await
        .expect("OAuth record");
    let core = spawn_internal(app_state(vault)).await;
    let handshake = internal_handshake(&core.base_url, &metadata.account_ref)
        .upgrade()
        .send()
        .await
        .expect("internal handshake");
    let mut socket = handshake.into_websocket().await.expect("internal socket");
    socket
        .send(DownstreamMessage::Text(
            serde_json::json!({
                "type":"response.create",
                "model":"gpt-5.4",
                "input":[{"type":"message","role":"user","content":[]}]
            })
            .to_string(),
        ))
        .await
        .expect("first create");
    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("completion timeout")
            .expect("completion")
            .expect("valid completion");
        if matches!(event, DownstreamMessage::Text(ref text) if text.contains("response.completed"))
        {
            break;
        }
    }

    let authorizations = state.authorizations.lock().await.clone();
    assert_eq!(
        authorizations,
        vec![
            format!("Bearer {}", state.old_access),
            format!("Bearer {}", state.new_access),
            format!("Bearer {}", state.new_access),
        ]
    );
    assert_eq!(state.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(state.accepted.load(Ordering::SeqCst), 2);
    let frames = state.frames.lock().await;
    assert_eq!(frames.len(), 2);
    let hidden: Value = serde_json::from_str(&frames[0]).expect("hidden frame");
    let public: Value = serde_json::from_str(&frames[1]).expect("public frame");
    assert_eq!(hidden["generate"], false);
    assert!(public.get("previous_response_id").is_none());
    let headers = state.accepted_headers.lock().await;
    assert_eq!(headers.len(), 2);
    let hidden_header = turn_metadata(&headers[0]);
    let public_header = turn_metadata(&headers[1]);
    let hidden_body = turn_metadata_from_body(&hidden);
    let public_body = turn_metadata_from_body(&public);
    assert_eq!(hidden_header["request_kind"], "prewarm");
    assert_eq!(public_header["request_kind"], "turn");
    assert_eq!(hidden_header, hidden_body);
    assert_eq!(public_header, public_body);
}

fn turn_metadata(headers: &HeaderMap) -> Value {
    serde_json::from_str(
        header_text(headers, "x-codex-turn-metadata")
            .as_deref()
            .expect("turn metadata header"),
    )
    .expect("turn metadata header JSON")
}

fn turn_metadata_from_body(body: &Value) -> Value {
    serde_json::from_str(
        body["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("body turn metadata"),
    )
    .expect("body turn metadata JSON")
}

async fn reconnect_oauth_upstream(
    AxumState(state): AxumState<ReconnectOAuthState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    let authorization =
        header_text(&headers, http::header::AUTHORIZATION.as_str()).unwrap_or_default();
    state
        .authorizations
        .lock()
        .await
        .push(authorization.clone());
    if authorization == format!("Bearer {}", state.old_access) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":{"code":"expired"}})),
        )
            .into_response();
    }
    state.accepted_headers.lock().await.push(headers);
    let connection = state.accepted.fetch_add(1, Ordering::SeqCst);
    upgrade
        .on_upgrade(move |mut socket| async move {
            let Some(Ok(InternalMessage::Text(frame))) = socket.next().await else {
                return;
            };
            state.frames.lock().await.push(frame.to_string());
            if connection == 0 {
                return;
            }
            for event in [
                r#"{"type":"response.created","response":{"id":"resp_public"}}"#,
                r#"{"type":"response.output_item.done","item":{"type":"message","role":"assistant","content":[]}}"#,
                r#"{"type":"response.completed","response":{"id":"resp_public","usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#,
            ] {
                let _ = socket.send(InternalMessage::Text(event.into())).await;
            }
        })
        .into_response()
}
