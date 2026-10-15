use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn reconnect_skips_hidden_prewarm_but_a_new_thread_keeps_it() {
    let capture = WebSocketCapture::default();
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", get(cadence_upstream))
            .with_state(capture.clone()),
    )
    .await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    let core = spawn_internal(state).await;
    for index in 0..3 {
        let handshake = internal_handshake(&core.base_url, &account)
            .upgrade()
            .send()
            .await
            .unwrap();
        let mut socket = handshake.into_websocket().await.unwrap();
        let mut body = serde_json::json!({"type":"response.create","model":"gpt-5.4",
            "input":[{"type":"message","role":"user","content":"synthetic task"}],
            "client_metadata":{"session_id":"synthetic-reconnect"}});
        if index == 2 {
            body["client_metadata"]["thread_id"] = "synthetic-child".into();
            body["client_metadata"]["parent_thread_id"] = "synthetic-reconnect".into();
        }
        socket
            .send(DownstreamMessage::Text(body.to_string()))
            .await
            .unwrap();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(3), socket.next())
                .await
                .expect("completion deadline")
                .expect("completion event")
                .unwrap();
            match event {
                DownstreamMessage::Text(text) if text.contains("response.completed") => break,
                DownstreamMessage::Close { .. } => panic!("closed before completion"),
                _ => {}
            }
        }
        socket
            .send(DownstreamMessage::Close {
                code: DownstreamCloseCode::Normal,
                reason: String::new(),
            })
            .await
            .unwrap();
        let frames = capture.frames.lock().await;
        let warmups = frames
            .iter()
            .filter(|frame| serde_json::from_str::<Value>(frame).unwrap()["generate"] == false)
            .count();
        assert_eq!(warmups, if index == 2 { 2 } else { 1 });
        assert_eq!(frames.len() - warmups, index + 1);
    }
}

async fn cadence_upstream(
    AxumState(capture): AxumState<WebSocketCapture>,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    upgrade
        .on_upgrade(move |mut socket| async move {
            while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
                let sequence = capture.calls.fetch_add(1, Ordering::SeqCst);
                capture.frames.lock().await.push(frame.to_string());
                let completed = serde_json::json!({"type":"response.completed", "response":{
                "id":format!("resp_synthetic_cadence_{sequence}"), "output":[]}});
                if socket
                    .send(InternalMessage::Text(completed.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        })
        .into_response()
}
