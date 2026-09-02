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
    BareOpenAi,
    CodexOpenAi149,
    CodexSubscription149,
}

impl UpstreamProfile {
    pub(crate) const fn select(caller: CallerKind, credential: CredentialKind) -> Self {
        match (caller, credential) {
            (CallerKind::Bare, CredentialKind::OpenAiApiKey) => Self::BareOpenAi,
            (CallerKind::Codex, CredentialKind::OpenAiApiKey) => Self::CodexOpenAi149,
            (_, CredentialKind::CodexSubscription) => Self::CodexSubscription149,
        }
    }

    pub(crate) const fn credential_kind(self) -> CredentialKind {
        match self {
            Self::BareOpenAi | Self::CodexOpenAi149 => CredentialKind::OpenAiApiKey,
            Self::CodexSubscription149 => CredentialKind::CodexSubscription,
        }
    }

    pub(crate) const fn emulates_codex(self) -> bool {
        !matches!(self, Self::BareOpenAi)
    }

    pub(crate) const fn uses_identity_state(self) -> bool {
        self.emulates_codex()
    }

    pub(crate) const fn uses_subscription_transport(self) -> bool {
        matches!(self, Self::CodexSubscription149)
    }

    pub(crate) const fn uses_oauth_refresh(self) -> bool {
        self.uses_subscription_transport()
    }

    pub(crate) const fn uses_http_zstd(self) -> bool {
        self.uses_subscription_transport()
    }

    pub(crate) const fn allows_openai_controls(self) -> bool {
        matches!(self, Self::CodexOpenAi149)
    }
}

#[cfg(test)]
#[path = "request_profile_tests.rs"]
mod tests;
