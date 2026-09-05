use super::*;
use pretty_assertions::assert_eq;

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
