use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn bare_api_key_route_relays_byte_exact_turns_and_filters_handshake_headers() {
    let capture = WebSocketCapture::default();
    let app = Router::new()
        .route("/responses", get(accepting_upstream))
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let vault = state.vault.clone();
    let core = spawn_internal(state).await;

    let handshake = internal_handshake(&core.base_url, &account_ref)
        .header("user-agent", "OpenAI/Go websocket-test")
        .header("openai-beta", "must-be-replaced")
        .header("openai-organization", "org-test")
        .header("cookie", "must-not-cross=1")
        .header("x-forwarded-for", "203.0.113.20")
        .upgrade()
        .send()
        .await
        .expect("internal handshake");

    assert_eq!(handshake.status(), StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(
        handshake.headers().get("x-models-etag").unwrap(),
        "etag-test"
    );
    assert!(!handshake.headers().contains_key("set-cookie"));
    assert!(!handshake.headers().contains_key("x-upstream-private"));
    assert!(handshake.headers().contains_key(CORE_TTFB_HEADER));
    let mut socket = handshake.into_websocket().await.expect("internal socket");

    let first = " {\"type\":\"response.create\", \"model\":\"first\"} ";
    let second =
        r#"{"type":"response.create","model":"second","previous_response_id":"resp_first"}"#;
    for (index, frame) in [first, second].into_iter().enumerate() {
        socket
            .send(DownstreamMessage::Text(frame.to_string()))
            .await
            .expect("send create frame");
        let event = socket
            .next()
            .await
            .expect("completion event")
            .expect("valid completion event");
        let DownstreamMessage::Text(event) = event else {
            panic!("expected text completion event");
        };
        let value: Value = serde_json::from_str(&event).expect("event JSON");
        assert_eq!(value["sequence"], index + 1);
    }
    let _ = socket.close(DownstreamCloseCode::Normal, None).await;

    assert_eq!(capture.frames.lock().await.as_slice(), [first, second]);
    assert!(
        !vault
            .request_state()
            .state_path_for_test(&account_ref)
            .exists(),
        "ApiKeyPassthrough WebSocket created request state"
    );
    let headers = capture
        .headers
        .lock()
        .await
        .clone()
        .expect("upstream headers");
    assert_eq!(
        header_text(&headers, http::header::AUTHORIZATION.as_str()).as_deref(),
        Some("Bearer upstream-websocket-api-key-test")
    );
    assert_eq!(
        header_text(&headers, "openai-beta").as_deref(),
        Some(crate::upstream_request::RESPONSES_WEBSOCKET_BETA)
    );
    assert_eq!(
        header_text(&headers, "openai-organization").as_deref(),
        Some("org-test")
    );
    assert_eq!(
        header_text(&headers, "sec-websocket-extensions").as_deref(),
        Some("permessage-deflate; client_max_window_bits")
    );
    for forbidden in [
        "cookie",
        "x-forwarded-for",
        mini_sub2api_protocol_v1::ACCOUNT_REF_HEADER,
    ] {
        assert!(!headers.contains_key(forbidden), "header {forbidden}");
    }
}

#[tokio::test]
async fn subscription_defers_provider_handshake_and_reuses_state_after_reconnect() {
    let capture = WebSocketCapture::default();
    let app = Router::new()
        .route("/responses", get(accepting_upstream))
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = subscription_state(&upstream.base_url).await;
    let vault = state.vault.clone();
    let core = spawn_internal(state).await;
    let create = serde_json::json!({
        "type":"response.create",
        "model":"gpt-5.4",
        "input":[{"type":"message","id":"msg_down","role":"user","content":"hello"}],
        "client_metadata":{
            "session_id":"session_down",
            "thread_id":"thread_down",
            "turn_id":"turn_down"
        }
    })
    .to_string();

    let mut first = internal_handshake(&core.base_url, &account_ref)
        .header("originator", "codex_exec")
        .upgrade()
        .send()
        .await
        .expect("first internal handshake")
        .into_websocket()
        .await
        .expect("first internal socket");
    assert_eq!(capture.calls.load(Ordering::SeqCst), 0);
    first
        .send(DownstreamMessage::Text(create.clone()))
        .await
        .expect("first create");
    let first_event = first
        .next()
        .await
        .expect("first completion")
        .expect("first completion event");
    let DownstreamMessage::Text(first_event) = first_event else {
        panic!("expected first completion text")
    };
    let first_event: Value = serde_json::from_str(&first_event).expect("first completion JSON");
    let first_response_id = first_event["response"]["id"]
        .as_str()
        .expect("first response alias")
        .to_string();
    assert_ne!(first_response_id, "resp_provider");
    assert_eq!(capture.calls.load(Ordering::SeqCst), 1);
    let first_frame: Value =
        serde_json::from_str(&capture.frames.lock().await[0]).expect("first projected frame");
    let first_metadata = &first_frame["client_metadata"];
    for (name, raw, version) in [
        ("session_id", "session_down", 7),
        ("thread_id", "thread_down", 7),
        ("turn_id", "turn_down", 7),
        (
            "x-codex-installation-id",
            "00000000-0000-0000-0000-000000000000",
            4,
        ),
    ] {
        let projected = first_metadata[name].as_str().expect("projected identity");
        assert_ne!(projected, raw);
        assert_eq!(
            uuid::Uuid::parse_str(projected)
                .expect("projected UUID")
                .get_version_num(),
            version
        );
    }
    let projected_session = first_metadata["session_id"]
        .as_str()
        .expect("projected session")
        .to_string();
    assert!(
        vault
            .request_state()
            .state_path_for_test("subscription-ws-test")
            .is_file()
    );
    first
        .close(DownstreamCloseCode::Normal, None)
        .await
        .expect("close first socket");

    let mut second = internal_handshake(&core.base_url, &account_ref)
        .header("originator", "codex_exec")
        .upgrade()
        .send()
        .await
        .expect("second internal handshake")
        .into_websocket()
        .await
        .expect("second internal socket");
    assert_eq!(capture.calls.load(Ordering::SeqCst), 1);
    second
        .send(DownstreamMessage::Text(create))
        .await
        .expect("second create");
    let second_event = second
        .next()
        .await
        .expect("second completion")
        .expect("second completion event");
    let DownstreamMessage::Text(second_event) = second_event else {
        panic!("expected second completion text")
    };
    let second_event: Value = serde_json::from_str(&second_event).expect("second completion JSON");
    assert_eq!(second_event["response"]["id"], first_response_id);
    assert_eq!(capture.calls.load(Ordering::SeqCst), 2);
    let second_frame: Value =
        serde_json::from_str(&capture.frames.lock().await[1]).expect("second projected frame");
    assert_eq!(
        second_frame["client_metadata"]["session_id"],
        projected_session
    );
}

#[tokio::test]
async fn subscription_websocket_compaction_commits_only_completed_terminal() {
    let capture = WebSocketCapture::default();
    let app = Router::new()
        .route("/responses", get(compaction_upstream))
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = subscription_state(&upstream.base_url).await;
    let core = spawn_internal(state).await;
    let mut socket = internal_handshake(&core.base_url, &account_ref)
        .header("originator", "codex_exec")
        .upgrade()
        .send()
        .await
        .expect("internal handshake")
        .into_websocket()
        .await
        .expect("internal socket");
    let compaction = |model: &str, turn: &str| {
        let metadata = serde_json::json!({
            "session_id":"compaction-session",
            "thread_id":"compaction-session",
            "turn_id":turn,
            "request_kind":"compaction",
            "compaction":{"trigger":"manual","implementation":"responses_compaction_v2"}
        });
        serde_json::json!({
            "type":"response.create",
            "model":model,
            "input":[{"type":"compaction_trigger"}],
            "client_metadata":{"x-codex-turn-metadata":metadata.to_string()}
        })
        .to_string()
    };
    for frame in [
        compaction("fail-compaction", "turn-one"),
        compaction("complete-compaction", "turn-one"),
        compaction("complete-compaction", "turn-two"),
    ] {
        socket
            .send(DownstreamMessage::Text(frame))
            .await
            .expect("send compaction");
        let event = socket
            .next()
            .await
            .expect("terminal event")
            .expect("valid terminal event");
        assert!(matches!(event, DownstreamMessage::Text(_)));
    }
    let frames = capture.frames.lock().await;
    let windows = frames
        .iter()
        .map(|frame| {
            serde_json::from_str::<Value>(frame)
                .expect("projected compaction")
                .get("client_metadata")
                .and_then(|metadata| metadata.get("x-codex-window-id"))
                .and_then(Value::as_str)
                .expect("projected compaction window")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert!(windows[0].ends_with(":0"));
    assert_eq!(windows[1], windows[0], "failed terminal advanced window");
    assert!(windows[2].ends_with(":1"));
}

#[tokio::test]
async fn upstream_rejection_stays_http_and_is_bounded_to_safe_metadata() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/responses",
        get({
            let calls = Arc::clone(&calls);
            move || {
                let calls = Arc::clone(&calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::UPGRADE_REQUIRED,
                        [
                            ("content-type", "application/json"),
                            ("set-cookie", "must-not-cross=1"),
                        ],
                        r#"{"error":{"code":"websocket_required"}}"#,
                    )
                }
            }
        }),
    );
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let core = spawn_internal(state).await;

    let rejection = internal_handshake(&core.base_url, &account_ref)
        .upgrade()
        .send()
        .await
        .expect("rejected internal handshake");
    assert_eq!(rejection.status(), StatusCode::UPGRADE_REQUIRED);
    assert!(!rejection.headers().contains_key("set-cookie"));
    let body = rejection.into_inner().text().await.expect("rejection body");
    assert_eq!(body, r#"{"error":{"code":"websocket_required"}}"#);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn invalid_internal_auth_never_reaches_upstream() {
    let capture = WebSocketCapture::default();
    let app = Router::new()
        .route("/responses", get(accepting_upstream))
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let core = spawn_internal(state).await;

    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client")
        .get(format!("{}/internal/v1/responses/ws", core.base_url))
        .header(http::header::AUTHORIZATION, "Bearer wrong-internal-token")
        .header(
            mini_sub2api_protocol_v1::VERSION_HEADER,
            mini_sub2api_protocol_v1::VERSION,
        )
        .header(mini_sub2api_protocol_v1::ACCOUNT_REF_HEADER, account_ref)
        .header(mini_sub2api_protocol_v1::REQUEST_ID_HEADER, "req_ws_test")
        .upgrade()
        .send()
        .await
        .expect("auth rejection");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(capture.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn loopback_websocket_uses_direct_client_and_enforces_message_limit() {
    let capture = WebSocketCapture::default();
    let app = Router::new()
        .route("/responses", get(accepting_upstream))
        .with_state(capture.clone());
    let upstream = spawn_loopback(app).await;
    let (mut state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    state.transports = Arc::new(
        crate::transport_registry::TransportRegistry::new_with_proxy_url("http://127.0.0.1:1")
            .expect("proxied transport registry"),
    );
    let core = spawn_internal(state).await;
    let handshake = internal_handshake(&core.base_url, &account_ref)
        .upgrade()
        .send()
        .await
        .expect("direct loopback handshake");
    let mut socket = handshake.into_websocket().await.expect("internal socket");
    let oversized = format!(
        "{{\"type\":\"response.create\",\"padding\":\"{}\"}}",
        "a".repeat(crate::inference_limits::get().request_bytes)
    );
    let send_result = socket.send(DownstreamMessage::Text(oversized)).await;
    if send_result.is_ok() {
        let closed = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("oversized close timeout");
        assert!(!matches!(closed, Some(Ok(DownstreamMessage::Text(_)))));
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(capture.frames.lock().await.is_empty());
}
