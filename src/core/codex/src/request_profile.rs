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

    pub(crate) const fn uses_codex_subscription(self) -> bool {
        matches!(self, Self::CodexSubscription149)
    }
}

#[cfg(test)]
#[path = "request_profile_tests.rs"]
mod tests;
