use super::*;

#[tokio::test]
async fn overlapping_public_creates_close_with_policy_violation() {
    let app = Router::new().route(
        "/responses",
        get(|upgrade: WebSocketUpgrade| async move {
            upgrade
                .on_upgrade(|mut socket| async move {
                    let _ = socket.next().await;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                })
                .into_response()
        }),
    );
    let upstream = spawn_loopback(app).await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let core = spawn_internal(state).await;
    let handshake = internal_handshake(&core.base_url, &account_ref)
        .upgrade()
        .send()
        .await
        .expect("internal handshake");
    let mut socket = handshake.into_websocket().await.expect("internal socket");

    for model in ["first", "overlap"] {
        socket
            .send(DownstreamMessage::Text(
                serde_json::json!({
                    "type": "response.create",
                    "model": model,
                    "input": []
                })
                .to_string(),
            ))
            .await
            .expect("send create");
    }

    let close = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("policy close timeout")
        .expect("policy close")
        .expect("valid close");
    let DownstreamMessage::Close { code, .. } = close else {
        panic!("expected policy close, got {close:?}");
    };
    assert!(u16::from(code) == 1008);
}
