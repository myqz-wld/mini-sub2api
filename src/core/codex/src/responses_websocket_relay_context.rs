use super::*;

pub(crate) struct RelayContext {
    pub(crate) headers: HeaderMap,
    pub(crate) account_ref: String,
    pub(crate) state_namespace: Option<String>,
    pub(crate) pseudonym_scope: String,
    pub(crate) profile: UpstreamProfile,
    pub(crate) continuation: ResponsesWebSocketState,
    pub(crate) pending: VecDeque<InternalMessage>,
    pub(crate) vault: Vault,
    pub(crate) fingerprint: FingerprintSnapshot,
    pub(crate) identity: Option<ResolvedRequestIdentity>,
    pub(crate) operation: Option<crate::subscription_context::Operation>,
}

#[derive(Clone, Copy)]
pub(super) enum RelayExit {
    Complete,
    StaleFingerprint,
    Failure(mini_sub2api_protocol_v1::FailureMetadata),
    StateUnavailable(mini_sub2api_protocol_v1::FailureMetadata),
    Policy,
    Protocol,
    TooLarge,
}
