use super::*;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

async fn startup_upstream(
    AxumState(capture): AxumState<FingerprintCapture>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    capture.calls.fetch_add(1, Ordering::SeqCst);
    capture.handshakes.lock().await.push(headers);
    upgrade.on_upgrade(move |mut socket| async move {
        while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
            let value: Value=serde_json::from_str(&frame).unwrap();
            capture.frames.lock().await.push(frame.to_string());
            let warm=value["generate"]==false;
            let id=if warm {"warm-before-refresh"} else {"business-after-refresh"};
            for kind in ["response.created","response.completed"] {
                if socket.send(InternalMessage::Text(json!({"type":kind,"response":{"id":id,"output":[]}}).to_string().into())).await.is_err() {return}
            }
            if warm && socket.send(InternalMessage::Text(json!({"type":"response.metadata","response_id":id,"headers":{"x-codex-turn-state":"old-startup-token"}}).to_string().into())).await.is_err() {return}
        }
    }).into_response()
}

async fn check_startup_handoff(rotate: bool) {
    let capture = FingerprintCapture::default();
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", get(startup_upstream))
            .with_state(capture.clone()),
    )
    .await;
    let temp = tempfile::tempdir().unwrap();
    let vault = Vault::open(temp.path().into()).unwrap();
    let metadata = vault
        .create_oauth(
            crate::vault::CredentialMaterial::CodexOAuth {
                id_token: crate::test_support::test_jwt(Some("startup-refresh-account"), 3600),
                access_token: crate::test_support::test_jwt(None, 3600),
                refresh_token: "synthetic-refresh".into(),
                account_id: "startup-refresh-account".into(),
                access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                issuer: upstream.base_url.clone(),
                client_id: "synthetic-client".into(),
            },
            format!("{}/responses", upstream.base_url),
            FingerprintMode::Device,
        )
        .await
        .unwrap();
    let core = spawn_internal(app_state(vault.clone())).await;
    let open = || {
        internal_handshake(&core.base_url, &metadata.account_ref)
            .header("originator", "codex_exec")
            .upgrade()
    };
    let mut socket = open().send().await.unwrap().into_websocket().await.unwrap();
    assert_eq!(capture.calls.load(Ordering::SeqCst), 1);
    assert!(capture.frames.lock().await.is_empty());
    let warm = json!({"type":"response.create","model":"gpt-5.4","generate":false,"input":[],"client_metadata":{"session_id":"refresh-session"}});
    socket
        .send(DownstreamMessage::Text(warm.to_string()))
        .await
        .unwrap();
    for kind in [
        "response.created",
        "response.completed",
        "response.metadata",
    ] {
        let DownstreamMessage::Text(text) = socket.next().await.unwrap().unwrap() else {
            panic!("expected startup event")
        };
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["type"], kind);
    }
    if rotate {
        let mut locked = vault.lock_record(&metadata.account_ref).await.unwrap();
        let crate::vault::CredentialMaterial::CodexOAuth { access_token, .. } =
            &mut locked.record.material
        else {
            unreachable!()
        };
        *access_token = crate::test_support::test_jwt(None, 7200);
        locked.persist().await.unwrap();
    }
    let business=json!({"type":"response.create","model":"gpt-5.4","input":[{"role":"user","content":"synthetic business"}],"client_metadata":{"session_id":"refresh-session","turn_id":"refresh-turn","x-codex-turn-state":"old-startup-token"}}).to_string();
    if rotate {
        socket
            .send(DownstreamMessage::Text(business.clone()))
            .await
            .unwrap();
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            DownstreamMessage::Close {
                code: DownstreamCloseCode::Restart,
                ..
            }
        ));
        assert_eq!(capture.frames.lock().await.len(), 1);
        socket = open().send().await.unwrap().into_websocket().await.unwrap();
    }
    socket
        .send(DownstreamMessage::Text(business))
        .await
        .unwrap();
    for _ in 0..2 {
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            DownstreamMessage::Text(_)
        ));
    }
    let frames = capture.frames.lock().await;
    assert_eq!(frames.len(), 2);
    let sent: Value = serde_json::from_str(&frames[1]).unwrap();
    assert_eq!(
        sent["client_metadata"]
            .get("x-codex-turn-state")
            .and_then(Value::as_str),
        if rotate {
            None
        } else {
            Some("old-startup-token")
        }
    );
    let handshakes = capture.handshakes.lock().await;
    let (canonical, probes): (Vec<_>, Vec<_>) = handshakes
        .iter()
        .partition(|headers| headers.contains_key("session-id"));
    let connections = if rotate { 2 } else { 1 };
    assert_eq!(canonical.len(), connections);
    assert_eq!(probes.len(), connections);
    assert_eq!(capture.calls.load(Ordering::SeqCst), handshakes.len());
    for probe in probes {
        for name in [
            "thread-id",
            "x-codex-installation-id",
            "x-codex-turn-metadata",
            "x-codex-window-id",
            "x-codex-guardian",
            "x-codex-routing-hint",
        ] {
            assert!(!probe.contains_key(name), "probe carried {name}");
        }
    }
}

#[tokio::test]
async fn refreshed_credentials_retire_startup_socket_without_token_handoff() {
    check_startup_handoff(true).await;
}

#[tokio::test]
async fn idle_startup_metadata_reaches_the_actual_business_frame() {
    check_startup_handoff(false).await;
}
