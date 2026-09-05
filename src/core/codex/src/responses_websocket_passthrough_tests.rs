use super::*;

#[tokio::test]
async fn all_api_key_callers_relay_exact_frames_and_responses_with_corrupt_state() {
    let capture = WebSocketCapture::default();
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", get(accepting_upstream))
            .with_state(capture.clone()),
    )
    .await;
    let (state, account_ref, _temp) = api_key_state(&upstream.base_url).await;
    let path = state
        .vault
        .request_state()
        .state_path_for_test(&account_ref);
    std::fs::write(&path, b"{corrupt").expect("broken identity state");
    let core = spawn_internal(state).await;
    let frame = r#" {"type":"response.create", "model":"gpt-5.6-sol", "previous_response_id":"unmapped", "input":[{"role":"system","content":"keep"}],"instructions":" {{caller}} ","tools":null,"future":true,"client_metadata":{"session_id":42}} "#;
    for codex in [false, true] {
        let request = internal_handshake(&core.base_url, &account_ref);
        let request = if codex {
            request.header("originator", "codex_exec")
        } else {
            request
        };
        let mut socket = request
            .upgrade()
            .send()
            .await
            .expect("handshake")
            .into_websocket()
            .await
            .expect("socket");
        socket
            .send(DownstreamMessage::Text(frame.to_string()))
            .await
            .expect("send");
        let event = socket.next().await.expect("event").expect("text");
        let DownstreamMessage::Text(event) = event else {
            panic!("expected text")
        };
        let value: Value = serde_json::from_str(&event).expect("response JSON");
        std::assert_eq!(value["response"]["id"], "resp_provider");
        std::assert_eq!(
            capture.frames.lock().await.last().map(String::as_str),
            Some(frame)
        );
        socket
            .close(DownstreamCloseCode::Normal, None)
            .await
            .expect("close");
    }
    std::assert_eq!(capture.calls.load(Ordering::SeqCst), 2);
    std::assert_eq!(std::fs::read(path).expect("state"), b"{corrupt");
}
