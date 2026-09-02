use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use mini_sub2api_protocol_v1::CoreError;
use mini_sub2api_protocol_v1::DeliveryState;
use mini_sub2api_protocol_v1::ErrorEnvelope;
use mini_sub2api_protocol_v1::FailureMetadata;
use mini_sub2api_protocol_v1::FailurePhase;
use mini_sub2api_protocol_v1::RetryAdvice;

#[derive(Debug, thiserror::Error)]
pub enum CoreFailure {
    #[error("invalid internal authentication")]
    InvalidInternalAuth,
    #[error("unsupported internal protocol")]
    UnsupportedProtocol,
    #[error("invalid request")]
    InvalidRequest,
    #[error("unknown account")]
    UnknownAccount,
    #[error("request identity state unavailable")]
    StateUnavailable,
    #[error("credential requires sign-in")]
    CredentialRequiresLogin,
    #[error("upstream connection failed")]
    UpstreamConnectFailed,
    #[error("upstream request delivery is unknown")]
    UpstreamDeliveryUnknown,
    #[error("upstream response handling failed")]
    UpstreamResponseFailed,
    #[error("upstream WebSocket handshake was rejected")]
    UpstreamHandshakeRejected,
    #[error("upstream authentication failed")]
    UpstreamAuthFailed,
    #[error("internal error")]
    Internal,
}

impl CoreFailure {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInternalAuth => "invalid_internal_auth",
            Self::UnsupportedProtocol => "unsupported_protocol",
            Self::InvalidRequest => "invalid_request",
            Self::UnknownAccount => "unknown_account",
            Self::StateUnavailable => "state_unavailable",
            Self::CredentialRequiresLogin => "credential_requires_login",
            Self::UpstreamConnectFailed => "upstream_connect_failed",
            Self::UpstreamDeliveryUnknown => "upstream_delivery_unknown",
            Self::UpstreamResponseFailed => "upstream_response_failed",
            Self::UpstreamHandshakeRejected => "upstream_handshake_rejected",
            Self::UpstreamAuthFailed => "upstream_auth_failed",
            Self::Internal => "internal_error",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::InvalidInternalAuth => StatusCode::UNAUTHORIZED,
            Self::UnsupportedProtocol | Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::UnknownAccount => StatusCode::NOT_FOUND,
            Self::StateUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::CredentialRequiresLogin | Self::UpstreamAuthFailed => StatusCode::UNAUTHORIZED,
            Self::UpstreamConnectFailed
            | Self::UpstreamDeliveryUnknown
            | Self::UpstreamResponseFailed
            | Self::UpstreamHandshakeRejected => StatusCode::BAD_GATEWAY,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn public_message(&self) -> &'static str {
        match self {
            Self::InvalidInternalAuth => "Internal authentication failed.",
            Self::UnsupportedProtocol => "The internal protocol version is unsupported.",
            Self::InvalidRequest => "The request is invalid.",
            Self::UnknownAccount => "The selected credential is unavailable.",
            Self::StateUnavailable => "The request identity state is unavailable.",
            Self::CredentialRequiresLogin => "The selected credential requires sign-in.",
            Self::UpstreamConnectFailed => "The upstream service is unavailable.",
            Self::UpstreamDeliveryUnknown => "The upstream request may have been delivered.",
            Self::UpstreamResponseFailed => "The upstream response could not be completed.",
            Self::UpstreamHandshakeRejected => "The upstream WebSocket handshake was rejected.",
            Self::UpstreamAuthFailed => "Upstream authentication failed.",
            Self::Internal => "The core encountered an internal error.",
        }
    }

    pub(crate) fn failure(&self) -> FailureMetadata {
        match self {
            Self::InvalidInternalAuth | Self::UnsupportedProtocol | Self::Internal => failure(
                RetryAdvice::Never,
                FailurePhase::Internal,
                DeliveryState::NotDelivered,
            ),
            Self::InvalidRequest => failure(
                RetryAdvice::Never,
                FailurePhase::Request,
                DeliveryState::NotDelivered,
            ),
            Self::UnknownAccount | Self::CredentialRequiresLogin => failure(
                RetryAdvice::Never,
                FailurePhase::Credential,
                DeliveryState::NotDelivered,
            ),
            Self::StateUnavailable => failure(
                RetryAdvice::Safe,
                FailurePhase::Internal,
                DeliveryState::NotDelivered,
            ),
            Self::UpstreamConnectFailed => failure(
                RetryAdvice::Safe,
                FailurePhase::UpstreamConnect,
                DeliveryState::NotDelivered,
            ),
            Self::UpstreamDeliveryUnknown => failure(
                RetryAdvice::Ambiguous,
                FailurePhase::UpstreamRequest,
                DeliveryState::PossiblyDelivered,
            ),
            Self::UpstreamAuthFailed | Self::UpstreamResponseFailed => failure(
                RetryAdvice::Never,
                FailurePhase::UpstreamResponse,
                DeliveryState::Delivered,
            ),
            Self::UpstreamHandshakeRejected => failure(
                RetryAdvice::Never,
                FailurePhase::UpstreamResponse,
                DeliveryState::NotDelivered,
            ),
        }
    }

    pub fn into_response(self, request_id: String) -> axum::response::Response {
        let status = self.status();
        let body = ErrorEnvelope {
            error: CoreError {
                code: self.code().to_string(),
                message: self.public_message().to_string(),
                request_id,
                failure: self.failure(),
            },
        };
        (status, Json(body)).into_response()
    }
}

pub(crate) const fn failure(
    retry_advice: RetryAdvice,
    phase: FailurePhase,
    delivery_state: DeliveryState,
) -> FailureMetadata {
    FailureMetadata {
        retry_advice,
        phase,
        delivery_state,
    }
}

#[cfg(test)]
mod tests {
    use super::CoreFailure;

    #[test]
    fn transport_failures_distinguish_connect_from_unknown_delivery() {
        assert_eq!(
            CoreFailure::UpstreamConnectFailed.failure(),
            super::failure(
                mini_sub2api_protocol_v1::RetryAdvice::Safe,
                mini_sub2api_protocol_v1::FailurePhase::UpstreamConnect,
                mini_sub2api_protocol_v1::DeliveryState::NotDelivered,
            )
        );
        assert_eq!(
            CoreFailure::UpstreamDeliveryUnknown.failure(),
            super::failure(
                mini_sub2api_protocol_v1::RetryAdvice::Ambiguous,
                mini_sub2api_protocol_v1::FailurePhase::UpstreamRequest,
                mini_sub2api_protocol_v1::DeliveryState::PossiblyDelivered,
            )
        );
    }

    #[test]
    fn every_core_failure_has_coherent_delivery_metadata() {
        for error in [
            CoreFailure::InvalidInternalAuth,
            CoreFailure::UnsupportedProtocol,
            CoreFailure::InvalidRequest,
            CoreFailure::UnknownAccount,
            CoreFailure::StateUnavailable,
            CoreFailure::CredentialRequiresLogin,
            CoreFailure::UpstreamConnectFailed,
            CoreFailure::UpstreamDeliveryUnknown,
            CoreFailure::UpstreamResponseFailed,
            CoreFailure::UpstreamHandshakeRejected,
            CoreFailure::UpstreamAuthFailed,
            CoreFailure::Internal,
        ] {
            assert!(error.failure().is_valid(), "{}", error.code());
        }
    }
}
