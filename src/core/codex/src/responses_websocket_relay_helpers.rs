use super::*;

pub(super) fn prepare_interrupt(
    continuation: &StdMutex<ResponsesWebSocketState>,
    text: &str,
) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    crate::response_interrupt::control_id(&value).is_ok_and(|id| {
        continuation_guard(continuation)
            .request_interrupt(id)
            .is_ok()
    })
}

pub(crate) async fn fingerprint_is_current(
    vault: &Vault,
    account_ref: &str,
    captured: &FingerprintSnapshot,
) -> bool {
    let Ok(current) = vault.fingerprint_snapshot(account_ref).await else {
        return false;
    };
    current.revision() == captured.revision() && current.mode() == captured.mode()
}

pub(super) fn public_create_in_flight(continuation: &StdMutex<ResponsesWebSocketState>) -> bool {
    matches!(
        continuation_guard(continuation).public_phase(),
        OperationPhase::Attempted | OperationPhase::ResponseObserved
    )
}

pub(super) fn observe_server_event(
    continuation: &StdMutex<ResponsesWebSocketState>,
    event: &Value,
) -> ObservedServerEvent {
    continuation_guard(continuation).observe_server_event_with_compaction(event)
}

pub(super) fn continuation_guard(
    continuation: &StdMutex<ResponsesWebSocketState>,
) -> MutexGuard<'_, ResponsesWebSocketState> {
    continuation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) fn upstream_close(code: UpstreamCloseCode) -> UpstreamMessage {
    UpstreamMessage::Close(Some(UpstreamCloseFrame {
        code,
        reason: "".into(),
    }))
}

pub(super) fn allowed_close_code(code: u16) -> UpstreamCloseCode {
    let code = UpstreamCloseCode::from(code);
    if code.is_allowed() {
        code
    } else {
        UpstreamCloseCode::Protocol
    }
}
