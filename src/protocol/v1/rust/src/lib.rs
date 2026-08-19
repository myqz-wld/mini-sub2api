use serde::Deserialize;
use serde::Serialize;

pub const VERSION: &str = "1";
pub const VERSION_HEADER: &str = "X-Mini-Sub2Api-Protocol-Version";
pub const ACCOUNT_REF_HEADER: &str = "X-Mini-Sub2Api-Account-Ref";
pub const REQUEST_ID_HEADER: &str = "X-Mini-Sub2Api-Request-Id";
pub const CORE_TTFB_HEADER: &str = "X-Mini-Sub2Api-Core-TTFB-Ms";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildIdentity {
    pub name: String,
    pub version: String,
    pub commit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    pub protocol_version: String,
    pub port: u16,
    pub pid: u32,
    pub build: BuildIdentity,
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
    pub retryable: bool,
    pub request_id: String,
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
                retryable: false,
                request_id: "req_01JEXAMPLE".to_string(),
            },
        };
        assert_eq!(got, want);
    }
}
