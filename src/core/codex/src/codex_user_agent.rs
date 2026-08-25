use crate::error::CoreFailure;
use crate::upstream_request::CODEX_COMPATIBILITY_VERSION;
use crate::upstream_request::CODEX_VERSION_HEADER;
use crate::upstream_request::DEFAULT_CODEX_ORIGINATOR;
use http::HeaderMap;
use http::HeaderValue;

const CANONICAL_PLATFORM_SUFFIX: &str = "(Ubuntu 22.4.0; x86_64) xterm-256color";

pub(crate) fn canonical_value() -> String {
    format!("{DEFAULT_CODEX_ORIGINATOR}/{CODEX_COMPATIBILITY_VERSION} {CANONICAL_PLATFORM_SUFFIX}")
}

/// Apply one complete, cross-machine subscription identity to both HTTP and WebSocket requests.
pub(crate) fn pin_subscription(headers: &mut HeaderMap) -> Result<(), CoreFailure> {
    let user_agent =
        HeaderValue::from_str(&canonical_value()).map_err(|_| CoreFailure::Internal)?;
    headers.insert(http::header::USER_AGENT, user_agent);
    headers.insert(
        "originator",
        HeaderValue::from_static(DEFAULT_CODEX_ORIGINATOR),
    );
    headers.insert(
        CODEX_VERSION_HEADER,
        HeaderValue::from_static(CODEX_COMPATIBILITY_VERSION),
    );
    Ok(())
}
