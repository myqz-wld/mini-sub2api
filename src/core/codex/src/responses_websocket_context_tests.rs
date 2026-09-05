use super::*;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

async fn context_upstream(
    AxumState(capture): AxumState<WebSocketCapture>,
    upgrade: WebSocketUpgrade,
) -> AxumResponse {
    upgrade.on_upgrade(move |mut socket| async move {
        while let Some(Ok(InternalMessage::Text(frame))) = socket.next().await {
            let mut frames = capture.frames.lock().await;
            let request: Value = serde_json::from_str(&frame).unwrap();
            let session = request["client_metadata"]["session_id"].clone();
            frames.push(frame.to_string());
            let n = frames.len(); drop(frames);
            let output = if n == 1 {
                json!({"type":"function_call","id":"fc_context_call","call_id":"call_context_tool","name":"example","arguments":"{}"})
            } else {
                json!({"type":"message","id":format!("msg_context_{n}"),"role":"assistant","content":[{"type":"output_text","text":"answer"}]})
            };
            let token = if n == 1 {"routing-first"} else {"routing-later"};
            for event in [
                json!({"type":"response.created","response":{"id":format!("resp_wscontext_{n}"),"session_id":session,"metadata":{"x-codex-turn-state":token}}}),
                json!({"type":"response.metadata","headers":{"x-codex-turn-state":token}}),
                json!({"type":"response.metadata","headers":{"x-codex-turn-state":"routing-ignored"}}),
                json!({"type":"response.output_item.done","output_index":0,"item":output}),
                json!({"type":"response.completed","response":{"id":format!("resp_wscontext_{n}"),"session_id":session,"output":[output],"metadata":{"x-codex-turn-state":"routing-ignored"}}}),
            ] {
                if socket.send(InternalMessage::Text(event.to_string().into())).await.is_err() { return; }
            }
        }
    }).into_response()
}

async fn completed(socket: &mut reqwest_websocket::WebSocket) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = socket
                .next()
                .await
                .expect("response event")
                .expect("valid event");
            let DownstreamMessage::Text(event) = event else {
                panic!("unexpected response frame");
            };
            let event: Value = serde_json::from_str(&event).expect("response JSON");
            if event["type"] == "response.completed" {
                return event["response"].clone();
            }
        }
    })
    .await
    .expect("completion deadline")
}

#[tokio::test]
async fn live_ws_continues_without_bulk_history_and_retains_only_its_turn_token() {
    for (model, native_lite) in [
        ("gpt-5.4", false),
        ("gpt-5.6-sol", false),
        ("gpt-5.6-sol", true),
        ("gpt-5.4", true),
    ] {
        let capture = WebSocketCapture::default();
        let upstream = spawn_loopback(
            Router::new()
                .route("/responses", get(context_upstream))
                .with_state(capture.clone()),
        )
        .await;
        let (state, account, _temp) = subscription_state(&upstream.base_url).await;
        let core = spawn_internal(state.clone()).await;
        let mut socket = internal_handshake(&core.base_url, &account)
            .header("originator", "codex_exec")
            .header(
                "x-codex-turn-metadata",
                r#"{"turn_id":"handshake-first-turn","window_id":"handshake-thread:0"}"#,
            )
            .upgrade()
            .send()
            .await
            .expect("handshake")
            .into_websocket()
            .await
            .expect("WS");
        let create = |previous: Option<&Value>, input: Value| {
            let mut value = json!({"type":"response.create","model":model,"input":input});
            if !native_lite {
                value["instructions"] = json!("caller base");
            }
            if let Some(previous) = previous {
                value["previous_response_id"] = previous["id"].clone();
                value["client_metadata"] = json!({"session_id":previous["session_id"]});
            } else if native_lite {
                let mut items = vec![
                    json!({"type":"additional_tools","role":"developer","tools":[]}),
                    json!({"type":"message","role":"developer","content":[{"type":"input_text","text":"native base"}]}),
                ];
                items.extend(value["input"].as_array().unwrap().iter().cloned());
                value["input"] = json!(items);
            }
            DownstreamMessage::Text(value.to_string())
        };
        socket
            .send(create(None, json!([{"role":"user","content":"first"}])))
            .await
            .expect("first create");
        let first = completed(&mut socket).await;
        state
            .vault
            .request_state()
            .contexts
            .inner
            .lock()
            .unwrap()
            .ttl = Duration::ZERO;
        socket.send(create(Some(&first), json!([{"type":"function_call_output","call_id":first["output"][0]["call_id"],"output":"done"}]))).await.expect("tool create");
        let second = completed(&mut socket).await;
        let http_delta = json!({"model":model,"previous_response_id":second["id"],"input":[]});
        let mut internal_headers = HeaderMap::new();
        internal_headers.insert(
            http::header::AUTHORIZATION,
            format!("Bearer {INTERNAL_TOKEN}").parse().unwrap(),
        );
        let error = crate::server::integration_support::call_core_with_headers(
            &state,
            &account,
            bytes::Bytes::from(serde_json::to_vec(&http_delta).unwrap()),
            internal_headers,
        )
        .await
        .expect_err("remote-only HTTP history");
        assert!(matches!(error, CoreFailure::StateUnavailable));
        socket
            .send(create(Some(&second), json!([])))
            .await
            .expect("empty create");
        let third = completed(&mut socket).await;
        socket
            .send(create(
                Some(&third),
                json!([{"role":"user","content":"new turn"}]),
            ))
            .await
            .expect("new turn create");
        completed(&mut socket).await;
        let frames = capture.frames.lock().await;
        assert_eq!(
            frames.len(),
            4,
            "unexpected inference replay or HTTP delivery"
        );
        let values: Vec<Value> = frames
            .iter()
            .map(|f| serde_json::from_str(f).unwrap())
            .collect();
        for (index, previous) in [
            (1, "resp_wscontext_1"),
            (2, "resp_wscontext_2"),
            (3, "resp_wscontext_3"),
        ] {
            assert_eq!(values[index]["previous_response_id"], previous);
        }
        for index in [1, 2] {
            assert_eq!(
                values[index]["client_metadata"]["x-codex-turn-state"],
                "routing-first"
            );
            assert_eq!(
                values[index]["client_metadata"]["turn_id"],
                values[0]["client_metadata"]["turn_id"]
            );
        }
        assert!(
            values[3]["client_metadata"]
                .get("x-codex-turn-state")
                .is_none()
        );
        assert_ne!(
            values[3]["client_metadata"]["turn_id"],
            values[0]["client_metadata"]["turn_id"]
        );
        assert_eq!(values[1]["input"][0]["call_id"], "call_context_tool");
        assert_eq!(values[1]["input"].as_array().unwrap().len(), 1);
        assert_ne!(first["id"], "resp_wscontext_1");
    }
}

#[tokio::test]
async fn handshake_selects_first_session_and_later_frame_cannot_mix_another_identity() {
    let capture = WebSocketCapture::default();
    let upstream = spawn_loopback(
        Router::new()
            .route("/responses", get(context_upstream))
            .with_state(capture.clone()),
    )
    .await;
    let (state, account, _temp) = subscription_state(&upstream.base_url).await;
    let core = spawn_internal(state).await;
    let mut socket = internal_handshake(&core.base_url, &account)
        .header("originator", "codex_exec")
        .header("session-id", "handshake-owner")
        .upgrade()
        .send()
        .await
        .unwrap()
        .into_websocket()
        .await
        .unwrap();
    let create = |session: Value| {
        DownstreamMessage::Text(
            json!({
                "type":"response.create","model":"gpt-5.4","instructions":"base",
                "input":[{"role":"user","content":"hello"}],
                "client_metadata":{"session_id":session}
            })
            .to_string(),
        )
    };
    socket
        .send(create(json!("lower-priority-first-frame")))
        .await
        .unwrap();
    let first = completed(&mut socket).await;
    socket
        .send(create(first["session_id"].clone()))
        .await
        .unwrap();
    completed(&mut socket).await;
    socket
        .send(create(json!("different-session")))
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("rejection deadline")
        .expect("close")
        .expect("valid close");
    assert!(matches!(event, DownstreamMessage::Close { code, .. } if u16::from(code) == 1002));
    assert_eq!(
        capture.frames.lock().await.len(),
        2,
        "conflicting frame reached inference"
    );
}
