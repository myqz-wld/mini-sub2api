use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TerminalKind {
    Completed,
    Failed,
    Incomplete,
}

impl TerminalKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => RESPONSE_TERMINAL_COMPLETED,
            Self::Failed => RESPONSE_TERMINAL_FAILED,
            Self::Incomplete => RESPONSE_TERMINAL_INCOMPLETE,
        }
    }
}

pub(super) struct TerminalResponse {
    pub(super) kind: TerminalKind,
    pub(super) response: serde_json::Value,
}

pub(super) fn terminal_response_from_sse(bytes: &[u8]) -> Result<TerminalResponse, CoreFailure> {
    let mut terminal = None;
    for event in events(bytes)? {
        let event = event?;
        let kind = match event.get("type").and_then(serde_json::Value::as_str) {
            Some("response.completed") => Some(TerminalKind::Completed),
            Some("response.failed") => Some(TerminalKind::Failed),
            Some("response.incomplete") => Some(TerminalKind::Incomplete),
            _ => None,
        };
        if let Some(kind) = kind
            && let Some(response) = event.get("response").cloned()
        {
            if terminal.is_some() {
                return Err(CoreFailure::UpstreamResponseFailed);
            }
            terminal = Some(TerminalResponse { kind, response });
        }
    }
    terminal.ok_or(CoreFailure::UpstreamResponseFailed)
}

pub(super) async fn prepare_terminal(
    bytes: &[u8],
    state: Option<&ResponseStateContext>,
) -> Result<TerminalResponse, CoreFailure> {
    // Validate the complete aggregation contract before publishing any context. Parsing one event
    // at a time avoids retaining a second collection of all response payloads.
    let mut terminal = terminal_response_from_sse(bytes)?;
    if let Some(state) = state {
        for event in events(bytes)? {
            let event = event?;
            match event.get("type").and_then(serde_json::Value::as_str) {
                Some("response.completed" | "response.failed" | "response.incomplete") => break,
                Some("response.created" | "response.metadata" | "response.output_item.done") => {
                    state
                        .translate_value(event)
                        .await
                        .map_err(|_| CoreFailure::UpstreamResponseFailed)?;
                }
                _ => {}
            }
        }
        terminal.response = state
            .translate_terminal_value(terminal.response, terminal.kind == TerminalKind::Completed)
            .await
            .map_err(|_| CoreFailure::UpstreamResponseFailed)?;
    }
    Ok(terminal)
}

fn events(
    bytes: &[u8],
) -> Result<impl Iterator<Item = Result<serde_json::Value, CoreFailure>> + '_, CoreFailure> {
    let text = std::str::from_utf8(bytes).map_err(|_| CoreFailure::UpstreamResponseFailed)?;
    let mut lines = text.lines().chain(std::iter::once(""));
    Ok(std::iter::from_fn(move || {
        let mut data = Vec::new();
        loop {
            let line = lines.next()?;
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                if data.is_empty() {
                    continue;
                }
                let payload = data.join("\n");
                data.clear();
                if payload == "[DONE]" {
                    continue;
                }
                return Some(
                    serde_json::from_str(&payload).map_err(|_| CoreFailure::UpstreamResponseFailed),
                );
            }
            if let Some(value) = line.strip_prefix("data:") {
                data.push(value.strip_prefix(' ').unwrap_or(value));
            }
        }
    }))
}
