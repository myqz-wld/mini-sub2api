use super::*;

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

pub(super) fn observe_server_text(
    continuation: &StdMutex<ResponsesWebSocketState>,
    text: &str,
) -> ObservedServerEvent {
    let mut continuation = continuation_guard(continuation);
    match serde_json::from_str::<Value>(text) {
        Ok(event) => continuation.observe_server_event_with_compaction(&event),
        Err(_) if public_phase_in_flight(continuation.public_phase()) => {
            continuation.fail_public_create();
            ObservedServerEvent {
                disposition: EventDisposition::ForwardPublic,
                completed_compaction: None,
            }
        }
        Err(_) => ObservedServerEvent {
            disposition: EventDisposition::Unassociated,
            completed_compaction: None,
        },
    }
}

fn public_phase_in_flight(phase: OperationPhase) -> bool {
    matches!(
        phase,
        OperationPhase::Attempted | OperationPhase::ResponseObserved
    )
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
