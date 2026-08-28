use crate::error::CoreFailure;
use crate::upstream_request::CODEX_COMPATIBILITY_VERSION;
use crate::upstream_request::CODEX_VERSION_HEADER;
use crate::upstream_request::DEFAULT_CODEX_ORIGINATOR;
use http::HeaderMap;
use http::HeaderValue;
use std::sync::LazyLock;

static PLATFORM_SUFFIX: LazyLock<String> = LazyLock::new(|| {
    let os = os_info::get();
    platform_suffix(
        &os.os_type().to_string(),
        &os.version().to_string(),
        os.architecture().unwrap_or("unknown"),
        &crate::terminal_detection::user_agent_token(),
    )
});

pub(crate) fn canonical_value() -> String {
    canonical_value_for_platform(PLATFORM_SUFFIX.as_str())
}

fn canonical_value_for_platform(platform_suffix: &str) -> String {
    sanitize_header_value(format!(
        "{DEFAULT_CODEX_ORIGINATOR}/{CODEX_COMPATIBILITY_VERSION} {platform_suffix} \
         ({DEFAULT_CODEX_ORIGINATOR}; {CODEX_COMPATIBILITY_VERSION})"
    ))
}

/// Apply one complete Codex identity derived from this deployment runtime.
pub(crate) fn pin_codex_identity(headers: &mut HeaderMap) -> Result<(), CoreFailure> {
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

fn platform_suffix(os_type: &str, version: &str, architecture: &str, terminal: &str) -> String {
    format!("({os_type} {version}; {architecture}) {terminal}")
}

fn sanitize_header_value(value: String) -> String {
    value
        .chars()
        .map(|character| {
            if matches!(character, ' '..='~') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_runtime_platform_like_codex() {
        assert_eq!(
            platform_suffix("Ubuntu", "24.4.0", "aarch64", "unknown"),
            "(Ubuntu 24.4.0; aarch64) unknown"
        );
    }

    #[test]
    fn formats_the_codex_tui_identity_like_codex_0149() {
        assert_eq!(
            canonical_value_for_platform("(Mac OS 15.6.1; arm64) Apple_Terminal"),
            "codex-tui/0.149.0 (Mac OS 15.6.1; arm64) Apple_Terminal \
             (codex-tui; 0.149.0)"
        );
    }

    #[test]
    fn sanitizes_dynamic_platform_values_for_http_headers() {
        assert_eq!(
            sanitize_header_value("codex/0.149.0 (Test\nOS; arch) terminal".to_string()),
            "codex/0.149.0 (Test_OS; arch) terminal"
        );
    }

    #[test]
    fn pins_the_complete_runtime_identity() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            HeaderValue::from_static("caller/9.9.9"),
        );
        headers.insert("originator", HeaderValue::from_static("caller"));
        headers.insert(CODEX_VERSION_HEADER, HeaderValue::from_static("9.9.9"));

        pin_codex_identity(&mut headers).expect("runtime identity");

        assert_eq!(
            headers
                .get(http::header::USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(canonical_value().as_str())
        );
        assert_eq!(
            headers
                .get("originator")
                .and_then(|value| value.to_str().ok()),
            Some(DEFAULT_CODEX_ORIGINATOR)
        );
        assert_eq!(
            headers
                .get(CODEX_VERSION_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some(CODEX_COMPATIBILITY_VERSION)
        );
    }
}
