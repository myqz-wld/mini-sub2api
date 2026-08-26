use super::*;
use crate::request_profile::CallerKind;
use crate::request_profile::UpstreamProfile;
use crate::responses_websocket_state::PublicCreateMode;
use crate::websocket_connector::AsyncIo;
use futures_util::SinkExt;
use futures_util::StreamExt;
use serde_json::json;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Copy)]
enum SetupReply {
    Completed,
    Failed,
    Silent,
}

#[tokio::test]
async fn successful_hidden_setup_establishes_an_incremental_public_baseline() {
    let (mut upstream, server) = connected_pair(SetupReply::Completed).await;
    let request = public_request();
    let mut state = automatic_subscription_state();
    let hidden = state
        .plan_hidden_setup(&request, PrewarmMode::Ordinary)
        .expect("hidden setup plan");

    let outcome = run_hidden_setup(
        &mut upstream,
        &mut state,
        serde_json::to_string(&hidden.frame).expect("hidden JSON"),
        Duration::from_secs(1),
    )
    .await;

    assert_eq!(outcome, HiddenSetupOutcome::Completed);
    assert_eq!(state.setup_phase(), OperationPhase::Completed);
    assert!(!state.public_create_attempted());
    let public = state.plan_public_create(&request);
    assert_eq!(public.mode, PublicCreateMode::Incremental);
    assert_eq!(public.frame["previous_response_id"], "resp_hidden");
    assert_eq!(public.frame["input"], request["input"]);
    server.await.expect("setup server");
}

#[tokio::test]
async fn failed_hidden_setup_is_consumed_and_public_create_falls_back_to_full() {
    let (mut upstream, server) = connected_pair(SetupReply::Failed).await;
    let request = public_request();
    let mut state = automatic_subscription_state();
    let hidden = state
        .plan_hidden_setup(&request, PrewarmMode::Ordinary)
        .expect("hidden setup plan");

    let outcome = run_hidden_setup(
        &mut upstream,
        &mut state,
        serde_json::to_string(&hidden.frame).expect("hidden JSON"),
        Duration::from_secs(1),
    )
    .await;

    assert_eq!(outcome, HiddenSetupOutcome::Failed);
    assert_eq!(state.setup_phase(), OperationPhase::Failed);
    assert!(!state.public_create_attempted());
    let public = state.plan_public_create(&request);
    assert_eq!(public.mode, PublicCreateMode::Full);
    assert!(public.frame.get("previous_response_id").is_none());
    assert_eq!(public.frame["input"], request["input"]);
    server.await.expect("setup server");
}

#[tokio::test]
async fn timed_out_hidden_setup_requests_reconnect_and_clears_reuse_state() {
    let (mut upstream, server) = connected_pair(SetupReply::Silent).await;
    let request = public_request();
    let mut state = automatic_subscription_state();
    let hidden = state
        .plan_hidden_setup(&request, PrewarmMode::Ordinary)
        .expect("hidden setup plan");

    let outcome = run_hidden_setup(
        &mut upstream,
        &mut state,
        serde_json::to_string(&hidden.frame).expect("hidden JSON"),
        Duration::from_millis(20),
    )
    .await;

    assert_eq!(outcome, HiddenSetupOutcome::Reconnect);
    assert_eq!(state.setup_phase(), OperationPhase::Failed);
    assert!(!state.public_create_attempted());
    state.reset_for_reconnect();
    let public = state.plan_public_create(&request);
    assert_eq!(public.mode, PublicCreateMode::Full);
    assert!(public.frame.get("previous_response_id").is_none());
    server.await.expect("setup server");
}

fn automatic_subscription_state() -> ResponsesWebSocketState {
    ResponsesWebSocketState::new(CallerKind::Bare, UpstreamProfile::CodexSubscription149)
}

fn public_request() -> Value {
    json!({
        "type": "response.create",
        "model": "gpt-5.4",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "bounded test"}]
        }],
        "stream": true
    })
}

async fn connected_pair(reply: SetupReply) -> (WebSocketConnection, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener");
    let address = listener.local_addr().expect("loopback address");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("loopback connection");
        let mut socket = accept_async(stream).await.expect("server handshake");
        let message = socket
            .next()
            .await
            .expect("hidden setup frame")
            .expect("valid hidden setup frame");
        let Message::Text(text) = message else {
            panic!("expected hidden text frame");
        };
        let frame: Value = serde_json::from_str(&text).expect("hidden setup JSON");
        assert_eq!(frame["generate"], false);
        match reply {
            SetupReply::Completed => {
                send_event(
                    &mut socket,
                    json!({"type":"response.created","response":{"id":"resp_hidden"}}),
                )
                .await;
                send_event(
                    &mut socket,
                    json!({"type":"response.completed","response":{"id":"resp_hidden","usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0}}}),
                )
                .await;
            }
            SetupReply::Failed => {
                send_event(
                    &mut socket,
                    json!({"type":"response.failed","response":{"id":"resp_hidden"}}),
                )
                .await;
            }
            SetupReply::Silent => {
                while let Some(message) = socket.next().await {
                    if matches!(message, Ok(Message::Close(_)) | Err(_)) {
                        break;
                    }
                }
            }
        }
    });

    let stream = TcpStream::connect(address)
        .await
        .expect("loopback client connection");
    let stream: Box<dyn AsyncIo> = Box::new(stream);
    let stream = MaybeTlsStream::Plain(stream);
    let url = format!("ws://{address}/responses");
    crate::test_support::assert_loopback_url(&url);
    let (socket, _) = client_async(url, stream).await.expect("client handshake");
    (socket, server)
}

async fn send_event<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>, event: Value)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    socket
        .send(Message::Text(event.to_string().into()))
        .await
        .expect("server event");
}
