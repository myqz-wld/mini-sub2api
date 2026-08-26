use crate::responses_websocket_state::EventDisposition;
use crate::responses_websocket_state::OperationPhase;
use crate::responses_websocket_state::PrewarmMode;
use crate::responses_websocket_state::ResponsesWebSocketState;
use crate::websocket_connector::WebSocketConnection;
use futures_util::SinkExt;
use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

pub(crate) const HIDDEN_SETUP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HiddenSetupOutcome {
    Completed,
    Failed,
    Reconnect,
}

pub(crate) fn prewarm_mode(request: &Value) -> PrewarmMode {
    let responses_lite = request
        .get("input")
        .and_then(Value::as_array)
        .and_then(|input| input.first())
        .and_then(Value::as_object)
        .and_then(|item| item.get("type"))
        .and_then(Value::as_str)
        == Some("additional_tools");
    if responses_lite {
        PrewarmMode::ResponsesLite
    } else {
        PrewarmMode::Ordinary
    }
}

pub(crate) async fn run_hidden_setup(
    upstream: &mut WebSocketConnection,
    continuation: &mut ResponsesWebSocketState,
    frame: String,
    timeout: Duration,
) -> HiddenSetupOutcome {
    if !continuation.mark_hidden_setup_attempted() {
        continuation.fail_hidden_setup();
        return HiddenSetupOutcome::Failed;
    }
    if upstream.send(Message::Text(frame.into())).await.is_err() {
        continuation.fail_hidden_setup();
        return HiddenSetupOutcome::Reconnect;
    }

    let outcome =
        tokio::time::timeout(timeout, consume_hidden_events(upstream, continuation)).await;
    match outcome {
        Ok(outcome) => outcome,
        Err(_) => {
            continuation.fail_hidden_setup();
            let _ = upstream
                .send(Message::Close(Some(CloseFrame {
                    code: CloseCode::Restart,
                    reason: "".into(),
                })))
                .await;
            HiddenSetupOutcome::Reconnect
        }
    }
}

async fn consume_hidden_events(
    upstream: &mut WebSocketConnection,
    continuation: &mut ResponsesWebSocketState,
) -> HiddenSetupOutcome {
    while let Some(message) = upstream.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let Ok(event) = serde_json::from_str::<Value>(&text) else {
                    continuation.fail_hidden_setup();
                    return HiddenSetupOutcome::Reconnect;
                };
                if continuation.observe_server_event(&event) != EventDisposition::ConsumeHiddenSetup
                {
                    continuation.fail_hidden_setup();
                    return HiddenSetupOutcome::Reconnect;
                }
                match continuation.setup_phase() {
                    OperationPhase::Completed => return HiddenSetupOutcome::Completed,
                    OperationPhase::Failed => return HiddenSetupOutcome::Failed,
                    _ => {}
                }
            }
            Ok(Message::Ping(payload)) => {
                if upstream.send(Message::Pong(payload)).await.is_err() {
                    continuation.fail_hidden_setup();
                    return HiddenSetupOutcome::Reconnect;
                }
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Binary(_)) | Ok(Message::Frame(_)) | Ok(Message::Close(_)) | Err(_) => {
                continuation.fail_hidden_setup();
                return HiddenSetupOutcome::Reconnect;
            }
        }
    }
    continuation.fail_hidden_setup();
    HiddenSetupOutcome::Reconnect
}

#[cfg(test)]
#[path = "responses_websocket_prewarm_tests.rs"]
mod tests;
