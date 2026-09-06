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
    let mut output = std::collections::BTreeMap::new();
    for event in events(bytes)? {
        let event = event?;
        if terminal.is_none()
            && event.get("type").and_then(serde_json::Value::as_str)
                == Some("response.output_item.done")
        {
            let index = match event.get("output_index") {
                None => output.len(),
                Some(value) => value
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or(CoreFailure::UpstreamResponseFailed)?,
            };
            let item = event
                .get("item")
                .filter(|item| item.is_object())
                .ok_or(CoreFailure::UpstreamResponseFailed)?
                .clone();
            if output.get(&index).is_some_and(|previous| previous != &item) {
                return Err(CoreFailure::UpstreamResponseFailed);
            }
            output.insert(index, item);
        }
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
    let mut terminal = terminal.ok_or(CoreFailure::UpstreamResponseFailed)?;
    // Codex's streamed terminal may contain response metadata without repeating output items.
    // A non-streaming Responses caller still needs the completed items in its JSON response.
    // A populated final output remains authoritative; never concatenate both representations.
    if crate::response_output::metadata_only(terminal.response.get("output")) {
        if !output.keys().copied().eq(0..output.len()) {
            return Err(CoreFailure::UpstreamResponseFailed);
        }
        terminal
            .response
            .as_object_mut()
            .ok_or(CoreFailure::UpstreamResponseFailed)?
            .insert(
                "output".into(),
                serde_json::Value::Array(output.into_values().collect()),
            );
    }
    Ok(terminal)
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

#[cfg(test)]
mod output_tests {
    use super::*;
    use serde_json::json;

    fn stream(events: Vec<serde_json::Value>) -> Vec<u8> {
        events
            .into_iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>()
            .into_bytes()
    }

    #[test]
    fn json_aggregation_recovers_items_from_metadata_only_completion() {
        let message = json!({"id":"msg_test","type":"message","role":"assistant","content":[{"type":"output_text","text":"PASS"}]});
        let reasoning =
            json!({"id":"rs_test","type":"reasoning","summary":[],"encrypted_content":"opaque"});
        for footer in [
            json!({"id":"resp_test","status":"completed"}),
            json!({"id":"resp_test","status":"completed","output":[]}),
        ] {
            let bytes = stream(vec![
                json!({"type":"response.output_item.done","output_index":1,"item":message}),
                json!({"type":"response.output_item.done","output_index":0,"item":reasoning}),
                json!({"type":"response.output_item.done","output_index":1,"item":message}),
                json!({"type":"response.completed","response":footer}),
            ]);
            let result = terminal_response_from_sse(&bytes).unwrap();
            assert_eq!(result.response["output"], json!([reasoning, message]));
        }
    }

    #[test]
    fn final_output_is_not_duplicated_or_replaced_by_observations() {
        let final_output = json!([{"type":"message","content":[]}]);
        let bytes = stream(vec![
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[]}}),
            json!({"type":"response.completed","response":{"id":"resp_test","output":final_output}}),
        ]);
        assert_eq!(
            terminal_response_from_sse(&bytes).unwrap().response["output"],
            final_output
        );
    }

    #[test]
    fn missing_final_output_requires_a_complete_consistent_item_sequence() {
        for events in [
            vec![
                json!({"type":"response.output_item.done","output_index":1,"item":{"type":"message"}}),
            ],
            vec![
                json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message"}}),
                json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning"}}),
            ],
        ] {
            let mut events = events;
            events.push(json!({"type":"response.completed","response":{"id":"resp_test"}}));
            assert!(terminal_response_from_sse(&stream(events)).is_err());
        }
    }
}
