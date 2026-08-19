use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use mini_sub2api_protocol_v1::CoreError;
use mini_sub2api_protocol_v1::ErrorEnvelope;

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
    #[error("credential requires sign-in")]
    CredentialRequiresLogin,
    #[error("upstream connection failed")]
    UpstreamConnectFailed,
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
            Self::CredentialRequiresLogin => "credential_requires_login",
            Self::UpstreamConnectFailed => "upstream_connect_failed",
            Self::UpstreamAuthFailed => "upstream_auth_failed",
            Self::Internal => "internal_error",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::InvalidInternalAuth => StatusCode::UNAUTHORIZED,
            Self::UnsupportedProtocol | Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::UnknownAccount => StatusCode::NOT_FOUND,
            Self::CredentialRequiresLogin | Self::UpstreamAuthFailed => StatusCode::UNAUTHORIZED,
            Self::UpstreamConnectFailed => StatusCode::BAD_GATEWAY,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn public_message(&self) -> &'static str {
        match self {
            Self::InvalidInternalAuth => "Internal authentication failed.",
            Self::UnsupportedProtocol => "The internal protocol version is unsupported.",
            Self::InvalidRequest => "The request is invalid.",
            Self::UnknownAccount => "The selected credential is unavailable.",
            Self::CredentialRequiresLogin => "The selected credential requires sign-in.",
            Self::UpstreamConnectFailed => "The upstream service is unavailable.",
            Self::UpstreamAuthFailed => "Upstream authentication failed.",
            Self::Internal => "The core encountered an internal error.",
        }
    }

    pub fn into_response(self, request_id: String) -> axum::response::Response {
        let status = self.status();
        let body = ErrorEnvelope {
            error: CoreError {
                code: self.code().to_string(),
                message: self.public_message().to_string(),
                retryable: matches!(self, Self::UpstreamConnectFailed),
                request_id,
            },
        };
        (status, Json(body)).into_response()
    }
}
