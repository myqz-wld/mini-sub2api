use super::*;

fn auth_hash(token: &str, account: Option<&str>, upstream: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update([u8::from(account.is_some())]);
    for value in [token, account.unwrap_or_default(), upstream] {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    }
    hash.finalize().into()
}

pub(crate) fn auth_binding(
    auth: &crate::upstream_request::ResolvedAuth,
    upstream: &str,
) -> [u8; 32] {
    use crate::upstream_request::ResolvedAuth;
    match auth {
        ResolvedAuth::CodexOAuth { token, account_id } => {
            auth_hash(token, Some(account_id), upstream)
        }
        ResolvedAuth::OpenAiApiKey { token } => auth_hash(token, None, upstream),
    }
}

pub(super) async fn identity_is_current(
    vault: &Vault,
    account: &str,
    fingerprint: &FingerprintSnapshot,
    binding: &[u8; 32],
) -> bool {
    use crate::vault::{CredentialMaterial, CredentialStatus};
    let Ok(locked) = vault.lock_record(account).await else {
        return false;
    };
    let current = locked.fingerprint();
    if current.revision() != fingerprint.revision()
        || current.mode() != fingerprint.mode()
        || locked.record.status != CredentialStatus::Ready
    {
        return false;
    }
    let upstream = &locked.record.upstream_url;
    let current = match &locked.record.material {
        CredentialMaterial::CodexOAuth {
            access_token,
            account_id,
            ..
        } => auth_hash(access_token, Some(account_id), upstream),
        CredentialMaterial::OpenAiApiKey { api_key } => auth_hash(api_key, None, upstream),
    };
    current == *binding
}

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
