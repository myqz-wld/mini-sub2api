use http::HeaderMap;
use serde_json::Value;

/// A WS handshake locates the session once. Its turn/window fields describe the first request.
/// Later frames supply their own turn evidence while retaining the handshake's session locator.
pub(crate) fn bound_headers(original: &HeaderMap) -> Result<HeaderMap, ()> {
    let mut headers = original.clone();
    for name in [
        "thread-id",
        "x-client-request-id",
        "x-codex-window-id",
        "x-codex-parent-thread-id",
    ] {
        headers.remove(name);
    }
    let sessions: Result<Vec<_>, ()> = original
        .get_all("x-codex-turn-metadata")
        .iter()
        .map(|header| {
            let value: Value =
                serde_json::from_str(header.to_str().map_err(|_| ())?).map_err(|_| ())?;
            let mut session = serde_json::Map::new();
            if let Some(value) = value.get("session_id") {
                session.insert("session_id".into(), value.clone());
            }
            crate::ascii_json::to_ascii_json_string(&Value::Object(session))
                .map_err(|_| ())?
                .parse()
                .map_err(|_| ())
        })
        .collect();
    headers.remove("x-codex-turn-metadata");
    for session in sessions? {
        headers.append("x-codex-turn-metadata", session);
    }
    Ok(headers)
}
