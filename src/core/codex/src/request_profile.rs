use http::HeaderMap;

const ORIGINATOR_HEADER: &str = "originator";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CallerKind {
    Bare,
    Codex,
}

impl CallerKind {
    pub(crate) fn from_headers(headers: &HeaderMap) -> Self {
        let is_codex = headers
            .get_all(ORIGINATOR_HEADER)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| !value.trim().is_empty());
        if is_codex { Self::Codex } else { Self::Bare }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CredentialKind {
    OpenAiApiKey,
    CodexSubscription,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpstreamProfile {
    ApiKeyPassthrough,
    CodexSubscription1534,
}

impl UpstreamProfile {
    pub(crate) const fn select(_caller: CallerKind, credential: CredentialKind) -> Self {
        match credential {
            CredentialKind::OpenAiApiKey => Self::ApiKeyPassthrough,
            CredentialKind::CodexSubscription => Self::CodexSubscription1534,
        }
    }

    pub(crate) const fn credential_kind(self) -> CredentialKind {
        match self {
            Self::ApiKeyPassthrough => CredentialKind::OpenAiApiKey,
            Self::CodexSubscription1534 => CredentialKind::CodexSubscription,
        }
    }

    pub(crate) const fn emulates_codex(self) -> bool {
        !matches!(self, Self::ApiKeyPassthrough)
    }

    pub(crate) const fn uses_identity_state(self) -> bool {
        self.emulates_codex()
    }

    pub(crate) const fn uses_subscription_transport(self) -> bool {
        matches!(self, Self::CodexSubscription1534)
    }

    pub(crate) const fn uses_oauth_refresh(self) -> bool {
        self.uses_subscription_transport()
    }

    pub(crate) const fn uses_http_zstd(self) -> bool {
        self.uses_subscription_transport()
    }
}

#[cfg(test)]
#[path = "request_profile_tests.rs"]
mod tests;
