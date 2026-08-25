use crate::error::CoreFailure;
use crate::error::failure;
use axum::extract::ws::CloseFrame;
use axum::extract::ws::Message;
use mini_sub2api_protocol_v1::DeliveryState;
use mini_sub2api_protocol_v1::FAILURE_CLOSE_CODE;
use mini_sub2api_protocol_v1::FailureMetadata;
use mini_sub2api_protocol_v1::FailurePhase;
use mini_sub2api_protocol_v1::RetryAdvice;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;

const DELIVERY_IDLE: u8 = 0;
const DELIVERY_ATTEMPTED: u8 = 1;
const DELIVERY_OBSERVED: u8 = 2;
const MAX_CLOSE_REASON_BYTES: usize = 123;
const INTERNAL_FAILURE_REASON: &str =
    r#"{"retryAdvice":"never","phase":"internal","deliveryState":"not_delivered"}"#;

#[derive(Default)]
pub(crate) struct WebSocketDeliveryTracker {
    state: AtomicU8,
}

impl WebSocketDeliveryTracker {
    pub(crate) fn mark_attempted(&self) {
        self.state.store(DELIVERY_ATTEMPTED, Ordering::Release);
    }

    pub(crate) fn mark_response_observed(&self) {
        if self.state.load(Ordering::Acquire) != DELIVERY_IDLE {
            self.state.store(DELIVERY_OBSERVED, Ordering::Release);
        }
    }

    pub(crate) fn mark_terminal(&self) {
        self.state.store(DELIVERY_IDLE, Ordering::Release);
    }

    pub(crate) fn failure(&self) -> FailureMetadata {
        match self.state.load(Ordering::Acquire) {
            DELIVERY_ATTEMPTED => failure(
                RetryAdvice::Ambiguous,
                FailurePhase::WebSocketRelay,
                DeliveryState::PossiblyDelivered,
            ),
            DELIVERY_OBSERVED => failure(
                RetryAdvice::Never,
                FailurePhase::WebSocketRelay,
                DeliveryState::Delivered,
            ),
            _ => failure(
                RetryAdvice::Safe,
                FailurePhase::WebSocketRelay,
                DeliveryState::NotDelivered,
            ),
        }
    }
}

pub(crate) fn failure_before_websocket_delivery(error: &CoreFailure) -> FailureMetadata {
    match error {
        CoreFailure::UpstreamAuthFailed => failure(
            RetryAdvice::Never,
            FailurePhase::Credential,
            DeliveryState::NotDelivered,
        ),
        CoreFailure::UpstreamHandshakeRejected | CoreFailure::UpstreamResponseFailed => failure(
            RetryAdvice::Never,
            FailurePhase::UpstreamResponse,
            DeliveryState::NotDelivered,
        ),
        _ => error.failure(),
    }
}

pub(crate) fn failure_close(metadata: FailureMetadata) -> Message {
    let reason = if metadata.is_valid() {
        serde_json::to_string(&metadata).unwrap_or_else(|_| INTERNAL_FAILURE_REASON.to_string())
    } else {
        INTERNAL_FAILURE_REASON.to_string()
    };
    let reason = if reason.len() <= MAX_CLOSE_REASON_BYTES {
        reason
    } else {
        INTERNAL_FAILURE_REASON.to_string()
    };
    Message::Close(Some(CloseFrame {
        code: FAILURE_CLOSE_CODE,
        reason: reason.into(),
    }))
}

pub(crate) fn internal_close(code: u16) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: "".into(),
    }))
}

pub(crate) fn is_terminal_response_event(text: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    matches!(
        value.get("type").and_then(serde_json::Value::as_str),
        Some("response.completed" | "response.failed" | "response.incomplete" | "error")
    )
}

pub(crate) fn is_response_create(text: &str) -> Result<bool, ()> {
    let value = serde_json::from_str::<serde_json::Value>(text).map_err(|_| ())?;
    let message_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .filter(|message_type| !message_type.is_empty())
        .ok_or(())?;
    Ok(message_type == "response.create")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_distinguishes_attempted_observed_and_idle() {
        let tracker = WebSocketDeliveryTracker::default();
        assert_eq!(tracker.failure().retry_advice, RetryAdvice::Safe);
        tracker.mark_attempted();
        assert_eq!(tracker.failure().retry_advice, RetryAdvice::Ambiguous);
        tracker.mark_response_observed();
        assert_eq!(tracker.failure().delivery_state, DeliveryState::Delivered);
        tracker.mark_terminal();
        assert_eq!(
            tracker.failure().delivery_state,
            DeliveryState::NotDelivered
        );
    }

    #[test]
    fn application_failure_close_is_bounded_json() {
        let Message::Close(Some(frame)) = failure_close(failure(
            RetryAdvice::Ambiguous,
            FailurePhase::WebSocketRelay,
            DeliveryState::PossiblyDelivered,
        )) else {
            panic!("failure close frame");
        };
        assert_eq!(frame.code, FAILURE_CLOSE_CODE);
        assert!(frame.reason.len() <= MAX_CLOSE_REASON_BYTES);
        let parsed: FailureMetadata = serde_json::from_str(&frame.reason).expect("failure JSON");
        assert!(parsed.is_valid());
    }
}
