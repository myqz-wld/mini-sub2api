//! Pinned Guardian v2 headers: provider, session/role extras, then transport/auth/defaults.
use super::*;

pub(super) const HTTP_ORDER: &[&str] = &[
    "version",
    "session-id",
    "thread-id",
    "x-codex-guardian",
    "x-openai-subagent",
    "x-codex-window-id",
    "x-openai-internal-codex-responses-lite",
    "x-client-request-id",
    "accept",
    "content-type",
    "authorization",
    "chatgpt-account-id",
    "originator",
    "user-agent",
    "x-openai-internal-codex-residency",
];

pub(super) fn websocket_headers(destination: &mut HeaderMap, source: &HeaderMap) {
    let mut headers = HeaderMap::new();
    copy_header(&mut headers, source, "version");
    for name in [
        "session-id",
        "thread-id",
        "x-codex-guardian",
        "x-openai-subagent",
        "x-codex-window-id",
        "x-openai-internal-codex-responses-lite",
        "x-client-request-id",
        "openai-beta",
        "originator",
        "user-agent",
        "x-openai-internal-codex-residency",
        "authorization",
        "chatgpt-account-id",
    ] {
        copy_header(&mut headers, source, name);
    }
    destination.extend(headers);
}
