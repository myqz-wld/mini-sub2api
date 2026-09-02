use serde::Deserialize;
use serde::Serialize;

pub const VERSION: &str = "1";
pub const VERSION_HEADER: &str = "X-Mini-Sub2Api-Protocol-Version";
pub const ACCOUNT_REF_HEADER: &str = "X-Mini-Sub2Api-Account-Ref";
pub const PSEUDONYM_SCOPE_HEADER: &str = "X-Mini-Sub2Api-Pseudonym-Scope";
pub const REQUEST_ID_HEADER: &str = "X-Mini-Sub2Api-Request-Id";
pub const CORE_TTFB_HEADER: &str = "X-Mini-Sub2Api-Core-TTFB-Ms";
pub const PROVIDER_REQUEST_ID_HEADER: &str = "X-Mini-Sub2Api-Provider-Request-Id";
pub const PROVIDER_REQUEST_ID_EVENT_TYPE: &str = "mini_sub2api.provider_request_id";
pub const RESPONSE_TERMINAL_HEADER: &str = "X-Mini-Sub2Api-Response-Terminal";
pub const RESPONSE_TERMINAL_COMPLETED: &str = "completed";
pub const RESPONSE_TERMINAL_FAILED: &str = "failed";
pub const RESPONSE_TERMINAL_INCOMPLETE: &str = "incomplete";
pub const FAILURE_PHASE_TRAILER: &str = "X-Mini-Sub2Api-Failure-Phase";
pub const DELIVERY_STATE_TRAILER: &str = "X-Mini-Sub2Api-Delivery-State";
pub const RETRY_ADVICE_TRAILER: &str = "X-Mini-Sub2Api-Retry-Advice";
pub const FAILURE_CLOSE_CODE: u16 = 4500;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryAdvice {
    Safe,
    Ambiguous,
    Never,
}

impl RetryAdvice {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Ambiguous => "ambiguous",
            Self::Never => "never",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePhase {
    Internal,
    Request,
    Credential,
    UpstreamConnect,
    UpstreamRequest,
    UpstreamResponse,
    UpstreamStream,
    WebSocketRelay,
}

impl FailurePhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Request => "request",
            Self::Credential => "credential",
            Self::UpstreamConnect => "upstream_connect",
            Self::UpstreamRequest => "upstream_request",
            Self::UpstreamResponse => "upstream_response",
            Self::UpstreamStream => "upstream_stream",
            Self::WebSocketRelay => "websocket_relay",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    NotDelivered,
    PossiblyDelivered,
    Delivered,
}

impl DeliveryState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotDelivered => "not_delivered",
            Self::PossiblyDelivered => "possibly_delivered",
            Self::Delivered => "delivered",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureMetadata {
    pub retry_advice: RetryAdvice,
    pub phase: FailurePhase,
    pub delivery_state: DeliveryState,
}

impl FailureMetadata {
    pub const fn is_valid(self) -> bool {
        matches!(
            (self.retry_advice, self.delivery_state),
            (RetryAdvice::Safe, DeliveryState::NotDelivered)
                | (RetryAdvice::Ambiguous, DeliveryState::PossiblyDelivered)
                | (RetryAdvice::Never, DeliveryState::NotDelivered)
                | (RetryAdvice::Never, DeliveryState::Delivered)
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildIdentity {
    pub name: String,
    pub version: String,
    pub commit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub responses_web_socket: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    pub protocol_version: String,
    pub port: u16,
    pub pid: u32,
    pub build: BuildIdentity,
    pub capabilities: Capabilities,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ErrorEnvelope {
    pub error: CoreError,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreError {
    pub code: String,
    pub message: String,
    pub request_id: String,
    #[serde(flatten)]
    pub failure: FailureMetadata,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRequestIdControl {
    #[serde(rename = "type")]
    pub event_type: String,
    pub provider_request_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_fixture_matches_contract() {
        let got: Readiness = serde_json::from_str(include_str!("../../fixtures/readiness.json"))
            .expect("readiness fixture");
        let want = Readiness {
            protocol_version: VERSION.to_string(),
            port: 42123,
            pid: 12345,
            build: BuildIdentity {
                name: "mini-sub2api-core-codex".to_string(),
                version: "0.1.0".to_string(),
                commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
            capabilities: Capabilities {
                responses_web_socket: true,
            },
        };
        assert_eq!(got, want);
    }

    #[test]
    fn error_fixture_matches_contract() {
        let got: ErrorEnvelope =
            serde_json::from_str(include_str!("../../fixtures/error.json")).expect("error fixture");
        let want = ErrorEnvelope {
            error: CoreError {
                code: "credential_requires_login".to_string(),
                message: "The selected credential requires sign-in.".to_string(),
                request_id: "req_01JEXAMPLE".to_string(),
                failure: FailureMetadata {
                    retry_advice: RetryAdvice::Never,
                    phase: FailurePhase::Credential,
                    delivery_state: DeliveryState::NotDelivered,
                },
            },
        };
        assert_eq!(got, want);
        assert!(got.error.failure.is_valid());
    }

    #[test]
    fn retry_advice_requires_a_coherent_delivery_state() {
        assert!(
            !FailureMetadata {
                retry_advice: RetryAdvice::Safe,
                phase: FailurePhase::UpstreamRequest,
                delivery_state: DeliveryState::PossiblyDelivered,
            }
            .is_valid()
        );
    }

    #[test]
    fn provider_request_id_control_is_bounded_to_the_private_shape() {
        let control = ProviderRequestIdControl {
            event_type: PROVIDER_REQUEST_ID_EVENT_TYPE.to_string(),
            provider_request_id: "provider-visible-ascii".to_string(),
        };
        assert_eq!(
            serde_json::to_string(&control).expect("control JSON"),
            r#"{"type":"mini_sub2api.provider_request_id","providerRequestId":"provider-visible-ascii"}"#
        );
    }

    #[test]
    fn response_terminal_header_contract_is_stable() {
        assert_eq!(RESPONSE_TERMINAL_HEADER, "X-Mini-Sub2Api-Response-Terminal");
        assert_eq!(RESPONSE_TERMINAL_COMPLETED, "completed");
        assert_eq!(RESPONSE_TERMINAL_FAILED, "failed");
        assert_eq!(RESPONSE_TERMINAL_INCOMPLETE, "incomplete");
    }
}
