use crate::error::CoreFailure;
use crate::error::failure;
use axum::extract::ws::CloseFrame;
use axum::extract::ws::Message;
use mini_sub2api_protocol_v1::DeliveryState;
use mini_sub2api_protocol_v1::FAILURE_CLOSE_CODE;
use mini_sub2api_protocol_v1::FailureMetadata;
use mini_sub2api_protocol_v1::FailurePhase;
use mini_sub2api_protocol_v1::RetryAdvice;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

const DELIVERY_IDLE: u64 = 0;
const DELIVERY_ATTEMPTED: u64 = 1;
const DELIVERY_OBSERVED: u64 = 2;
const MAX_CLOSE_REASON_BYTES: usize = 123;
const INTERNAL_FAILURE_REASON: &str =
    r#"{"retryAdvice":"never","phase":"internal","deliveryState":"not_delivered"}"#;

#[derive(Default)]
pub(crate) struct WebSocketDeliveryTracker {
    // Low two bits are delivery state; the rest identify the public create.
    state: AtomicU64,
    retired: AtomicBool,
}

impl WebSocketDeliveryTracker {
    pub(crate) fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        self.mark_response_observed();
    }

    pub(crate) fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }

    pub(crate) fn mark_attempted(&self) {
        let _ = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some((state & !3).wrapping_add(4) | DELIVERY_ATTEMPTED)
            });
    }

    pub(crate) fn mark_response_observed(&self) {
        let _ = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state & 3 != DELIVERY_IDLE).then_some((state & !3) | DELIVERY_OBSERVED)
            });
    }

    pub(crate) fn generation(&self) -> u64 {
        self.state.load(Ordering::Acquire) >> 2
    }

    pub(crate) fn mark_terminal(&self, generation: u64) {
        let _ = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state >> 2 == generation).then_some(state & !3)
            });
    }

    pub(crate) fn failure(&self) -> FailureMetadata {
        self.failure_for_phase(FailurePhase::WebSocketRelay)
    }

    pub(crate) fn failure_for_phase(&self, phase: FailurePhase) -> FailureMetadata {
        if self.is_retired() {
            return failure(RetryAdvice::Never, phase, DeliveryState::Delivered);
        }
        match self.state.load(Ordering::Acquire) & 3 {
            DELIVERY_ATTEMPTED => failure(
                RetryAdvice::Ambiguous,
                phase,
                DeliveryState::PossiblyDelivered,
            ),
            DELIVERY_OBSERVED => failure(RetryAdvice::Never, phase, DeliveryState::Delivered),
            _ => failure(RetryAdvice::Safe, phase, DeliveryState::NotDelivered),
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
        CoreFailure::UpstreamHandshakeRejected
        | CoreFailure::UpstreamResponseFailed
        | CoreFailure::FlexUnavailable
        | CoreFailure::UpstreamInvalidPrompt
        | CoreFailure::NativeResponse(_, _) => failure(
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
        assert_eq!(
            tracker.failure_for_phase(FailurePhase::Internal),
            failure(
                RetryAdvice::Never,
                FailurePhase::Internal,
                DeliveryState::Delivered,
            )
        );
        tracker.mark_terminal(tracker.generation());
        assert_eq!(
            tracker.failure().delivery_state,
            DeliveryState::NotDelivered
        );
    }

    #[test]
    fn late_terminal_never_resets_a_new_create_delivery_proof() {
        let tracker = WebSocketDeliveryTracker::default();
        tracker.mark_attempted();
        tracker.mark_response_observed();
        let failed = tracker.generation();
        tracker.mark_attempted();
        tracker.mark_terminal(failed);
        assert_eq!(tracker.failure().retry_advice, RetryAdvice::Ambiguous);
        tracker.mark_response_observed();
        tracker.mark_terminal(failed);
        assert_eq!(tracker.failure().delivery_state, DeliveryState::Delivered);
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
