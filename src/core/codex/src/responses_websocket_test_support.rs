use super::*;

pub(super) async fn accepting_upstream(
    AxumState(capture): AxumState<WebSocketCapture>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    capture.calls.fetch_add(1, Ordering::SeqCst);
    *capture.headers.lock().await = Some(headers);
    let relay_capture = capture.clone();
    let mut response = upgrade
        .max_message_size(crate::inference_limits::get().request_bytes)
        .max_frame_size(crate::inference_limits::get().request_bytes)
        .on_upgrade(move |mut socket| async move {
            let mut sequence = 0;
            while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
                sequence += 1;
                relay_capture.frames.lock().await.push(frame.to_string());
                let event = serde_json::json!({
                    "type": "response.completed",
                    "sequence": sequence,
                    "response": {
                        "id":"resp_provider",
                        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                    }
                })
                .to_string();
                if socket
                    .send(InternalMessage::Text(event.into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        })
        .into_response();
    response
        .headers_mut()
        .insert("x-models-etag", HeaderValue::from_static("etag-test"));
    response
        .headers_mut()
        .insert("set-cookie", HeaderValue::from_static("must-not-cross=1"));
    response.headers_mut().insert(
        "x-upstream-private",
        HeaderValue::from_static("must-not-cross"),
    );
    response
}

pub(super) async fn compaction_upstream(
    AxumState(capture): AxumState<WebSocketCapture>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    capture.calls.fetch_add(1, Ordering::SeqCst);
    *capture.headers.lock().await = Some(headers);
    let relay_capture = capture.clone();
    upgrade
        .max_message_size(crate::inference_limits::get().request_bytes)
        .max_frame_size(crate::inference_limits::get().request_bytes)
        .on_upgrade(move |mut socket| async move {
            while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
                relay_capture.frames.lock().await.push(frame.to_string());
                let value: Value = serde_json::from_str(&frame).expect("compaction frame JSON");
                let completed = value["model"] == "complete-compaction";
                let event = serde_json::json!({
                    "type": if completed { "response.completed" } else { "response.failed" },
                    "response":{"id": if completed { "resp_completed" } else { "resp_failed" }}
                })
                .to_string();
                if socket
                    .send(InternalMessage::Text(event.into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        })
        .into_response()
}

pub(super) async fn subscription_state(base_url: &str) -> (AppState, String, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_oauth(
            crate::vault::CredentialMaterial::CodexOAuth {
                id_token: crate::test_support::test_jwt(Some("subscription-ws-test"), 7200),
                access_token: crate::test_support::test_jwt(None, 7200),
                refresh_token: "offline-refresh".to_string(),
                account_id: "subscription-ws-test".to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(2)),
                issuer: base_url.to_string(),
                client_id: "offline-client".to_string(),
            },
            format!("{base_url}/responses"),
            FingerprintMode::Device,
        )
        .await
        .expect("subscription credential");
    (app_state(vault), metadata.account_ref, temp)
}

pub(super) async fn api_key_state(base_url: &str) -> (AppState, String, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_api_key(
            "upstream-websocket-api-key-test".to_string(),
            format!("{base_url}/responses"),
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("API key record");
    let state = app_state(vault);
    (state, metadata.account_ref, temp)
}

pub(super) fn app_state(vault: Vault) -> AppState {
    AppState {
        vault,
        transports: Arc::new(
            crate::transport_registry::TransportRegistry::new().expect("transport registry"),
        ),
        internal_token_hash: Sha256::digest(INTERNAL_TOKEN.as_bytes()).into(),
        account_locks: Arc::new(Mutex::new(HashMap::new())),
    }
}

pub(super) async fn spawn_internal(state: AppState) -> RunningInternalServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind internal test server");
    let address = listener.local_addr().expect("internal test address");
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            internal_router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve internal test server");
    });
    RunningInternalServer {
        base_url: format!("http://{address}"),
        task,
    }
}

pub(super) fn internal_handshake(base_url: &str, account_ref: &str) -> reqwest::RequestBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("internal client")
        .get(format!("{base_url}/internal/v1/responses/ws"))
        .header(
            http::header::AUTHORIZATION,
            format!("Bearer {INTERNAL_TOKEN}"),
        )
        .header(
            mini_sub2api_protocol_v1::VERSION_HEADER,
            mini_sub2api_protocol_v1::VERSION,
        )
        .header(mini_sub2api_protocol_v1::ACCOUNT_REF_HEADER, account_ref)
        .header(
            mini_sub2api_protocol_v1::PSEUDONYM_SCOPE_HEADER,
            PSEUDONYM_SCOPE,
        )
        .header(mini_sub2api_protocol_v1::REQUEST_ID_HEADER, "req_ws_test")
}
