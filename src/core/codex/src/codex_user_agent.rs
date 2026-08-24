use crate::error::CoreFailure;
use crate::upstream_request::CODEX_COMPATIBILITY_VERSION;
use crate::upstream_request::DEFAULT_CODEX_ORIGINATOR;
use http::HeaderMap;
use http::HeaderValue;
use std::sync::LazyLock;

static PLATFORM_SUFFIX: LazyLock<String> = LazyLock::new(|| {
    let os = os_info::get();
    let terminal = crate::terminal_detection::user_agent_token();
    format!(
        "({} {}; {}) {terminal}",
        os.os_type(),
        os.version(),
        os.architecture().unwrap_or("unknown")
    )
});

pub(crate) fn pin(headers: &mut HeaderMap) -> Result<(), CoreFailure> {
    let anchored = headers
        .get(http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .and_then(anchor_existing)
        .unwrap_or_else(|| fallback(headers));
    let value = HeaderValue::from_str(&anchored).map_err(|_| CoreFailure::Internal)?;
    headers.insert(http::header::USER_AGENT, value);
    Ok(())
}

fn fallback(headers: &HeaderMap) -> String {
    let originator = headers
        .get("originator")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_CODEX_ORIGINATOR);
    for_originator(originator)
}

pub(crate) fn for_originator(originator: &str) -> String {
    format!(
        "{originator}/{CODEX_COMPATIBILITY_VERSION} {}",
        PLATFORM_SUFFIX.as_str()
    )
}

pub(crate) fn anchor_existing(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let (product, version_and_suffix) = raw.split_once('/')?;
    let (version, suffix) = version_and_suffix
        .split_once(' ')
        .map_or((version_and_suffix, ""), |(version, suffix)| {
            (version, suffix)
        });
    let normalized_product = product.to_ascii_lowercase();
    if version.is_empty()
        || !(normalized_product == "codex"
            || normalized_product.starts_with("codex_")
            || normalized_product.starts_with("codex-")
            || normalized_product.starts_with("codex "))
    {
        return None;
    }
    let suffix = if suffix.is_empty() {
        String::new()
    } else {
        format!(" {suffix}")
    };
    Some(format!("{product}/{CODEX_COMPATIBILITY_VERSION}{suffix}"))
}
