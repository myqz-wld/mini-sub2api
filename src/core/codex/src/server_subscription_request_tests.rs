use super::*;

#[tokio::test]
async fn subscription_route_normalizes_plain_request_and_preserves_client_tools() {
    let capture = ApiCapture::default();
    let app = Router::new()
        .route(
            "/responses",
            axum_post(
                |AxumState(capture): AxumState<ApiCapture>,
                 headers: HeaderMap,
                 body: Bytes| async move {
                    capture.calls.fetch_add(1, Ordering::SeqCst);
                    *capture.headers.lock().await = Some(headers);
                    *capture.body.lock().await = Some(body);
                    (StatusCode::OK, "normalized")
                },
            ),
        )
        .with_state(capture.clone());
    let mock = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let account_id = "chatgpt-normalizer-test";
    let access_token = test_jwt(None, 3600);
    let metadata = vault
        .create_oauth(
            CredentialMaterial::CodexOAuth {
                id_token: test_jwt(Some(account_id), 3600),
                access_token: access_token.clone(),
                refresh_token: "refresh-normalizer-test".to_string(),
                account_id: account_id.to_string(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                issuer: mock.base_url.clone(),
                client_id: "client-normalizer-test".to_string(),
            },
            format!("{}/responses", mock.base_url),
            crate::fingerprint::FingerprintMode::Device,
        )
        .await
        .expect("OAuth record");
    let tools = serde_json::json!([
        {"type":"function","name":"lookup","description":"Lookup","parameters":{"type":"object"}},
        {"type":"web_search_preview"}
    ]);
    let body = Bytes::from(
        serde_json::to_vec(&serde_json::json!({
            "model": "gpt-5.6-sol",
            "instructions": "Be concise",
            "input": "hello",
            "tools": tools,
            "stream": true,
            "max_output_tokens": 32768,
            "client_metadata": {
                "x-codex-installation-id": "body-device-conflict",
                "x-codex-turn-metadata": serde_json::json!({
                    "installation_id": "body-turn-device-conflict",
                    "session_id": "body-session-kept",
                    "thread_id": "body-thread-kept",
                    "turn_id": "body-turn-kept",
                    "window_id": "body-window-kept",
                    "future": {"kept": true}
                }).to_string()
            }
        }))
        .expect("request body"),
    );
    let mut extra_headers = HeaderMap::new();
    for (name, value) in [
        ("originator", "codex_exec"),
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
        ("x-codex-installation-id", "header-device-conflict"),
        (
            "x-codex-turn-metadata",
            r#"{"installation_id":"header-turn-device-conflict","session_id":"header-session-kept","future":1}"#,
        ),
        ("x-openai-internal-codex-responses-lite", "true"),
        ("openai-organization", "must-not-cross"),
        ("openai-project", "must-not-cross"),
        ("x-stainless-lang", "must-not-cross"),
    ] {
        extra_headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_str(value).expect("header value"),
        );
    }

    let state = app_state(vault);
    let response = call_core_with_headers(&state, &metadata.account_ref, body, extra_headers)
        .await
        .expect("core response");

    assert_eq!(response.status(), StatusCode::OK);
    let captured_body = capture.body.lock().await.clone().expect("captured body");
    let captured_body = zstd::stream::decode_all(std::io::Cursor::new(captured_body.as_ref()))
        .expect("decompress normalized request");
    let normalized: serde_json::Value =
        serde_json::from_slice(&captured_body).expect("normalized request");
    let expected_device = normalized["client_metadata"]["x-codex-installation-id"]
        .as_str()
        .expect("installation")
        .to_string();
    assert_eq!(
        uuid::Uuid::parse_str(&expected_device)
            .expect("installation UUID")
            .get_version_num(),
        4
    );
    assert_eq!(normalized["input"][0]["type"], "additional_tools");
    assert_eq!(normalized["input"][0]["tools"][0]["type"], "namespace");
    assert_eq!(normalized["input"][0]["tools"][0]["name"], "functions");
    assert_eq!(
        normalized["input"][0]["tools"][0]["tools"][0]["name"],
        "lookup"
    );
    assert_eq!(normalized["input"][0]["tools"][1], tools[1]);
    assert_eq!(normalized["input"][1]["role"], "developer");
    assert_eq!(normalized["input"][1]["content"][0]["text"], "Be concise");
    assert_eq!(normalized["input"][2]["role"], "user");
    assert_eq!(normalized["input"].as_array().expect("input").len(), 3);
    assert_eq!(normalized["store"], false);
    assert_eq!(normalized["stream"], true);
    assert!(normalized.get("max_output_tokens").is_none());
    assert!(
        normalized["client_metadata"]["x-codex-installation-id"].as_str()
            == Some(expected_device.as_str())
    );
    let body_turn: serde_json::Value = serde_json::from_str(
        normalized["client_metadata"]["x-codex-turn-metadata"]
            .as_str()
            .expect("body turn metadata"),
    )
    .expect("body turn JSON");
    assert!(body_turn["installation_id"].as_str() == Some(expected_device.as_str()));
    for (name, raw) in [
        ("session_id", "body-session-kept"),
        ("thread_id", "body-thread-kept"),
        ("turn_id", "body-turn-kept"),
    ] {
        let pseudonym = body_turn[name].as_str().expect("pseudonym");
        assert_ne!(pseudonym, raw);
        assert_eq!(
            uuid::Uuid::parse_str(pseudonym)
                .expect("pseudonym UUID")
                .get_version_num(),
            7
        );
    }
    assert_eq!(body_turn["session_id"], body_turn["thread_id"]);
    assert_eq!(
        body_turn["window_id"],
        format!("{}:0", body_turn["thread_id"].as_str().expect("thread"))
    );
    assert!(body_turn.get("future").is_none());
    let captured_headers = capture.headers.lock().await.clone().expect("headers");
    assert_eq!(
        header_text(&captured_headers, http::header::AUTHORIZATION.as_str()).as_deref(),
        Some(format!("Bearer {access_token}").as_str())
    );
    assert_eq!(
        header_text(&captured_headers, "chatgpt-account-id").as_deref(),
        Some(account_id)
    );
    assert_eq!(
        header_text(&captured_headers, http::header::USER_AGENT.as_str()).as_deref(),
        Some(crate::codex_user_agent::canonical_value().as_str())
    );
    assert!(!captured_headers.contains_key("x-codex-installation-id"));
    assert_eq!(
        header_text(&captured_headers, "content-encoding").as_deref(),
        Some("zstd")
    );
    assert_eq!(
        header_text(&captured_headers, "x-codex-routing-hint").as_deref(),
        Some("model=gpt-5.6-sol")
    );
    let header_turn: serde_json::Value = serde_json::from_str(
        header_text(&captured_headers, "x-codex-turn-metadata")
            .as_deref()
            .expect("header turn metadata"),
    )
    .expect("header turn JSON");
    assert!(header_turn["installation_id"].as_str() == Some(expected_device.as_str()));
    assert_ne!(header_turn["session_id"], "header-session-kept");
    assert!(header_turn.get("future").is_none());
    for (name, expected) in [
        ("originator", "codex-tui"),
        ("x-openai-internal-codex-responses-lite", "true"),
    ] {
        assert_eq!(
            header_text(&captured_headers, name).as_deref(),
            Some(expected)
        );
    }
    for (name, raw) in [("session-id", "session-test"), ("thread-id", "thread-test")] {
        let pseudonym = header_text(&captured_headers, name).expect("identity header");
        assert_ne!(pseudonym, raw);
        assert_eq!(
            uuid::Uuid::parse_str(&pseudonym)
                .expect("pseudonym UUID")
                .get_version_num(),
            7
        );
    }
    for name in ["openai-organization", "openai-project", "x-stainless-lang"] {
        assert!(!captured_headers.contains_key(name), "header {name}");
    }
    assert!(!captured_headers.contains_key(PSEUDONYM_SCOPE_HEADER));
}
