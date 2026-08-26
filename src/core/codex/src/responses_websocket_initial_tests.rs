use super::*;
use crate::test_support::test_jwt;
use crate::vault::CredentialMaterial;
use tokio::sync::Notify;

#[derive(Clone, Default)]
struct InitialFenceCapture {
    frames: Arc<Mutex<Vec<String>>>,
    hidden_seen: Arc<Notify>,
    release_hidden: Arc<Notify>,
    hidden_completed: Arc<Notify>,
}

#[derive(Clone, Copy)]
enum DownstreamTerminal {
    Close,
    Overlap,
}

#[tokio::test]
async fn deferred_initial_rechecks_downstream_after_fingerprint_wait() {
    for terminal in [DownstreamTerminal::Close, DownstreamTerminal::Overlap] {
        let capture = InitialFenceCapture::default();
        let upstream = spawn_loopback(
            Router::new()
                .route("/responses", get(initial_fence_upstream))
                .with_state(capture.clone()),
        )
        .await;
        let temp = tempfile::tempdir().expect("tempdir");
        let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
        let account_id = "initial-fence-account";
        let metadata = vault
            .create_oauth(
                CredentialMaterial::CodexOAuth {
                    id_token: test_jwt(Some(account_id), 3600),
                    access_token: test_jwt(None, 3600),
                    refresh_token: "initial-fence-refresh".to_string(),
                    account_id: account_id.to_string(),
                    access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                    issuer: upstream.base_url.clone(),
                    client_id: "initial-fence-client".to_string(),
                },
                format!("{}/responses", upstream.base_url),
                FingerprintMode::Off,
            )
            .await
            .expect("OAuth record");
        let core = spawn_internal(app_state(vault.clone())).await;
        let handshake = internal_handshake(&core.base_url, &metadata.account_ref)
            .upgrade()
            .send()
            .await
            .expect("internal handshake");
        let mut socket = handshake.into_websocket().await.expect("internal socket");
        let create = serde_json::json!({
            "type":"response.create",
            "model":"gpt-5.4",
            "input":[]
        })
        .to_string();
        socket
            .send(DownstreamMessage::Text(create.clone()))
            .await
            .expect("first create");
        tokio::time::timeout(Duration::from_secs(2), capture.hidden_seen.notified())
            .await
            .expect("hidden setup");

        let locked = vault
            .lock_record(&metadata.account_ref)
            .await
            .expect("hold credential record lock");
        capture.release_hidden.notify_one();
        tokio::time::timeout(Duration::from_secs(2), capture.hidden_completed.notified())
            .await
            .expect("hidden completion");
        tokio::time::sleep(Duration::from_millis(30)).await;
        let overlap = matches!(terminal, DownstreamTerminal::Overlap);
        match terminal {
            DownstreamTerminal::Close => socket
                .send(DownstreamMessage::Close {
                    code: DownstreamCloseCode::Normal,
                    reason: String::new(),
                })
                .await
                .expect("downstream close"),
            DownstreamTerminal::Overlap => {
                socket
                    .send(DownstreamMessage::Text(create))
                    .await
                    .expect("overlap create");
            }
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        drop(locked);
        if overlap {
            let event = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await
                .expect("overlap close timeout")
                .expect("overlap close")
                .expect("valid overlap close");
            assert!(matches!(
                event,
                DownstreamMessage::Close {
                    code: DownstreamCloseCode::Policy,
                    ..
                }
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        pretty_assertions::assert_eq!(
            capture.frames.lock().await.len(),
            1,
            "public create crossed the final downstream liveness fence"
        );
    }
}

async fn initial_fence_upstream(
    AxumState(capture): AxumState<InitialFenceCapture>,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    upgrade
        .on_upgrade(move |mut socket| async move {
            let Some(Ok(InternalMessage::Text(hidden))) = socket.next().await else {
                return;
            };
            capture.frames.lock().await.push(hidden.to_string());
            capture.hidden_seen.notify_one();
            capture.release_hidden.notified().await;
            for event in [
                r#"{"type":"response.created","response":{"id":"resp_hidden"}}"#,
                r#"{"type":"response.completed","response":{"id":"resp_hidden","usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0}}}"#,
            ] {
                if socket
                    .send(InternalMessage::Text(event.into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            capture.hidden_completed.notify_one();
            while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
                capture.frames.lock().await.push(frame.to_string());
            }
        })
        .into_response()
}
