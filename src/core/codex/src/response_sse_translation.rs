use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::Stream;
#[cfg(test)]
use futures_util::StreamExt;
use http_body::Frame;
use mini_sub2api_protocol_v1::DELIVERY_STATE_TRAILER;
use mini_sub2api_protocol_v1::DeliveryState;
use mini_sub2api_protocol_v1::FAILURE_PHASE_TRAILER;
use mini_sub2api_protocol_v1::FailurePhase;
use mini_sub2api_protocol_v1::RETRY_ADVICE_TRAILER;
use mini_sub2api_protocol_v1::RetryAdvice;
use std::convert::Infallible;

use crate::error::failure;
use crate::response_translation::{ResponseStateContext, SseFailureTail};

pub(crate) use crate::response_sse_reader::UpstreamByteStream;
use crate::response_sse_reader::{SseReader, data_payload};

struct TranslationState {
    reader: SseReader,
    context: Option<ResponseStateContext>,
    finished: bool,
    terminal_seen: bool,
    failed_footer_pending: bool,
    failure_tail: Option<SseFailureTail>,
    maximum: usize,
    output_lifecycle: crate::response_output::OutputLifecycle,
}

impl Drop for TranslationState {
    fn drop(&mut self) {
        self.reader.close();
        self.failure_tail = None;
        if let Some(context) = self.context.take() {
            let _ = context.update_operation(None);
        }
    }
}

pub(crate) fn translated_sse_frames(
    upstream: UpstreamByteStream,
    context: ResponseStateContext,
    maximum: usize,
) -> impl Stream<Item = Result<Frame<Bytes>, Infallible>> {
    translate_reader(SseReader::new(upstream, maximum), context, maximum)
}

pub(crate) fn translate_reader(
    reader: SseReader,
    context: ResponseStateContext,
    maximum: usize,
) -> impl Stream<Item = Result<Frame<Bytes>, Infallible>> {
    futures_util::stream::unfold(
        TranslationState {
            reader,
            context: Some(context),
            finished: false,
            terminal_seen: false,
            failed_footer_pending: false,
            failure_tail: None,
            maximum,
            output_lifecycle: Default::default(),
        },
        |mut state| async move {
            if state.finished {
                return None;
            }
            match state.reader.next_event().await {
                Ok(Some(event)) => Some(finish_event(state, event).await),
                Ok(None) if state.terminal_seen => None,
                Ok(None) | Err(_) => Some(fail(state)),
            }
        },
    )
}

async fn finish_event(
    mut state: TranslationState,
    event: Vec<u8>,
) -> (Result<Frame<Bytes>, Infallible>, TranslationState) {
    match translate_event(&mut state, event).await {
        Ok(bytes) => (Ok(Frame::data(bytes)), state),
        Err(()) => fail(state),
    }
}

fn fail(mut state: TranslationState) -> (Result<Frame<Bytes>, Infallible>, TranslationState) {
    state.finished = true;
    state.reader.close();
    state.failure_tail = None;
    if let Some(context) = state.context.take() {
        let _ = context.update_operation(None);
    }
    let metadata = failure(
        RetryAdvice::Never,
        FailurePhase::UpstreamStream,
        DeliveryState::Delivered,
    );
    let mut trailers = HeaderMap::new();
    trailers.insert(
        FAILURE_PHASE_TRAILER,
        metadata
            .phase
            .as_str()
            .parse()
            .expect("static failure phase"),
    );
    trailers.insert(
        DELIVERY_STATE_TRAILER,
        metadata
            .delivery_state
            .as_str()
            .parse()
            .expect("static delivery state"),
    );
    trailers.insert(
        RETRY_ADVICE_TRAILER,
        metadata
            .retry_advice
            .as_str()
            .parse()
            .expect("static retry advice"),
    );
    (Ok(Frame::trailers(trailers)), state)
}

async fn translate_event(state: &mut TranslationState, event: Vec<u8>) -> Result<Bytes, ()> {
    let text = std::str::from_utf8(&event).map_err(|_| ())?;
    let data = data_payload(text);
    let Some(data) = data else {
        return Ok(Bytes::from(event));
    };
    if data.trim().is_empty() || data.trim() == "[DONE]" {
        return Ok(Bytes::from(event));
    }
    let value: serde_json::Value = serde_json::from_str(&data).map_err(|_| ())?;
    if state.terminal_seen && crate::response_output::OutputLifecycle::is_output_event(&value) {
        return Err(());
    }
    let kind = value.get("type").and_then(serde_json::Value::as_str);
    let response_terminal = matches!(
        kind,
        Some("response.completed" | "response.failed" | "response.incomplete")
    );
    let failed_footer = state.failed_footer_pending && kind == Some("response.failed");
    if state.terminal_seen && response_terminal && !failed_footer {
        return Err(());
    }
    if kind == Some("error") && !state.terminal_seen {
        state.failure_tail = state
            .context
            .as_ref()
            .expect("live translation context")
            .detach_sse_failure()
            .map_err(|_| ())?;
        state.failed_footer_pending = true;
    }
    state
        .output_lifecycle
        .observe(&value, crate::inference_limits::get().output_items)
        .map_err(|_| ())?;
    if kind == Some("response.completed") {
        state
            .output_lifecycle
            .validate_completed(&value["response"])
            .map_err(|_| ())?;
    }
    let terminal = response_terminal || kind == Some("error");
    let tail = (failed_footer || kind == Some("error"))
        .then_some(state.failure_tail.as_ref())
        .flatten();
    let translated = state
        .context
        .as_ref()
        .expect("live translation context")
        .translate_sse_value(value, tail)
        .await
        .map_err(|_| ())?;
    let translated = serde_json::to_string(&translated).map_err(|_| ())?;
    let rewritten = replace_data_lines(text, &translated)?;
    if rewritten.len() > state.maximum {
        return Err(());
    }
    state.terminal_seen |= terminal;
    if failed_footer {
        state.failed_footer_pending = false;
        state.failure_tail = None;
    }
    Ok(Bytes::from(rewritten))
}

fn replace_data_lines(event: &str, translated: &str) -> Result<String, ()> {
    let mut output = String::with_capacity(event.len().max(translated.len() + 16));
    let mut replaced = false;
    for segment in event.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let content = line.strip_suffix('\r').unwrap_or(line);
        if content.starts_with("data:") {
            if !replaced {
                output.push_str("data: ");
                output.push_str(translated);
                if segment.ends_with("\r\n") {
                    output.push_str("\r\n");
                } else if segment.ends_with('\n') {
                    output.push('\n');
                }
                replaced = true;
            }
        } else {
            output.push_str(segment);
        }
    }
    if !replaced {
        return Err(());
    }
    Ok(output)
}

#[cfg(test)]
fn find_event_end(bytes: &[u8]) -> Option<usize> {
    let mut line_start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let line = &bytes[line_start..index];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            return Some(index + 1);
        }
        line_start = index + 1;
    }
    None
}

#[cfg(test)]
#[path = "response_sse_translation_tests.rs"]
mod tests;
