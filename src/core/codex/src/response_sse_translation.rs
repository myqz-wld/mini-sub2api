use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::Stream;
use futures_util::StreamExt;
use http_body::Frame;
use mini_sub2api_protocol_v1::DELIVERY_STATE_TRAILER;
use mini_sub2api_protocol_v1::DeliveryState;
use mini_sub2api_protocol_v1::FAILURE_PHASE_TRAILER;
use mini_sub2api_protocol_v1::FailurePhase;
use mini_sub2api_protocol_v1::RETRY_ADVICE_TRAILER;
use mini_sub2api_protocol_v1::RetryAdvice;
use std::convert::Infallible;
use std::pin::Pin;

use crate::error::failure;
use crate::response_translation::ResponseStateContext;

pub(crate) type UpstreamByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

struct TranslationState {
    upstream: UpstreamByteStream,
    context: ResponseStateContext,
    buffer: Vec<u8>,
    delivered: bool,
    finished: bool,
    maximum: usize,
}

pub(crate) fn translated_sse_frames(
    upstream: UpstreamByteStream,
    context: ResponseStateContext,
    maximum: usize,
) -> impl Stream<Item = Result<Frame<Bytes>, Infallible>> {
    futures_util::stream::unfold(
        TranslationState {
            upstream,
            context,
            buffer: Vec::new(),
            delivered: false,
            finished: false,
            maximum,
        },
        |mut state| async move {
            if state.finished {
                return None;
            }
            loop {
                if let Some(end) = find_event_end(&state.buffer) {
                    let event = state.buffer.drain(..end).collect::<Vec<_>>();
                    return Some(finish_event(state, event).await);
                }
                match state.upstream.next().await {
                    Some(Ok(bytes)) => {
                        if append_bounded(&mut state.buffer, &bytes, state.maximum).is_err() {
                            return Some(fail(state));
                        }
                    }
                    Some(Err(_)) => return Some(fail(state)),
                    None if state.buffer.is_empty() => return None,
                    None => {
                        let event = std::mem::take(&mut state.buffer);
                        state.finished = true;
                        return Some(finish_event(state, event).await);
                    }
                }
            }
        },
    )
}

async fn finish_event(
    mut state: TranslationState,
    event: Vec<u8>,
) -> (Result<Frame<Bytes>, Infallible>, TranslationState) {
    match translate_event(&state.context, event, state.maximum).await {
        Ok(bytes) => {
            state.delivered = true;
            (Ok(Frame::data(bytes)), state)
        }
        Err(()) => fail(state),
    }
}

fn fail(mut state: TranslationState) -> (Result<Frame<Bytes>, Infallible>, TranslationState) {
    state.finished = true;
    let delivery_state = if state.delivered {
        DeliveryState::Delivered
    } else {
        DeliveryState::NotDelivered
    };
    let metadata = failure(
        RetryAdvice::Never,
        FailurePhase::UpstreamStream,
        delivery_state,
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

async fn translate_event(
    context: &ResponseStateContext,
    event: Vec<u8>,
    maximum: usize,
) -> Result<Bytes, ()> {
    let text = std::str::from_utf8(&event).map_err(|_| ())?;
    let data = data_payload(text);
    let Some(data) = data else {
        return Ok(Bytes::from(event));
    };
    if data.trim().is_empty() || data.trim() == "[DONE]" {
        return Ok(Bytes::from(event));
    }
    let translated = context
        .translate_text(data, maximum)
        .await
        .map_err(|_| ())?;
    let rewritten = replace_data_lines(text, &translated)?;
    if rewritten.len() > maximum {
        return Err(());
    }
    Ok(Bytes::from(rewritten))
}

fn data_payload(event: &str) -> Option<String> {
    let parts = event
        .lines()
        .filter_map(|line| {
            line.strip_suffix('\r')
                .unwrap_or(line)
                .strip_prefix("data:")
        })
        .map(|value| value.strip_prefix(' ').unwrap_or(value))
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("\n"))
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

fn append_bounded(destination: &mut Vec<u8>, chunk: &[u8], maximum: usize) -> Result<(), ()> {
    if destination
        .len()
        .checked_add(chunk.len())
        .is_none_or(|length| length > maximum)
    {
        return Err(());
    }
    destination.extend_from_slice(chunk);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_state_types::WireIdDomain;
    use crate::request_wire_ids::translate_request_ids;
    use std::collections::BTreeSet;

    #[test]
    fn detects_lf_and_crlf_event_boundaries() {
        assert_eq!(find_event_end(b"data: {}\n\nnext"), Some(10));
        assert_eq!(find_event_end(b"data: {}\r\n\r\nnext"), Some(12));
        assert_eq!(find_event_end(b"data: {}\n"), None);
    }

    #[test]
    fn replaces_multiline_data_and_preserves_event_fields() {
        let event = "event: response.completed\r\nid: 7\r\ndata: {\"type\":\r\ndata: \"response.completed\"}\r\n\r\n";
        assert_eq!(
            data_payload(event).as_deref(),
            Some("{\"type\":\n\"response.completed\"}")
        );
        let got =
            replace_data_lines(event, "{\"type\":\"response.completed\"}").expect("replace data");
        assert!(got.contains("event: response.completed\r\n"));
        assert!(got.contains("id: 7\r\n"));
        assert_eq!(got.matches("data:").count(), 1);
    }

    #[tokio::test]
    async fn split_sse_event_persists_aliases_before_delivery_and_round_trips() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = crate::request_state_store::RequestStateStore::new(temp.path().to_path_buf());
        store
            .edit(
                "namespace-sse",
                "acct_sse_translation",
                "scope-sse",
                |editor| {
                    editor.bind_wire_pair(
                        WireIdDomain::Response,
                        "resp_downstream",
                        "resp_upstream",
                    )?;
                    editor.bind_wire_pair(
                        WireIdDomain::Turn,
                        "turn_downstream",
                        "turn_upstream",
                    )?;
                    Ok(())
                },
            )
            .await
            .expect("seed mappings");
        let context =
            ResponseStateContext::new("acct_sse_translation", "namespace-sse", "scope-sse", &store);
        let event = concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_upstream\",",
            "\"output\":[{\"type\":\"function_call\",\"id\":\"item_provider\",",
            "\"call_id\":\"call_provider\",\"arguments\":\"{\\\"opaque_id\\\":\\\"keep\\\"}\",",
            "\"internal_chat_message_metadata_passthrough\":{\"turn_id\":\"turn_upstream\"}}]}}\n\n"
        );
        let split = event.len() / 2;
        let upstream: UpstreamByteStream = Box::pin(futures_util::stream::iter(vec![
            Ok(Bytes::copy_from_slice(&event.as_bytes()[..split])),
            Ok(Bytes::copy_from_slice(&event.as_bytes()[split..])),
        ]));
        let frames = translated_sse_frames(upstream, context, 1024 * 1024)
            .collect::<Vec<_>>()
            .await;
        assert_eq!(frames.len(), 1);
        let bytes = frames
            .into_iter()
            .next()
            .expect("frame")
            .expect("infallible")
            .into_data()
            .expect("data frame");
        assert!(store.state_path_for_test("namespace-sse").is_file());
        let text = std::str::from_utf8(&bytes).expect("translated SSE");
        let payload = data_payload(text).expect("SSE data");
        let value: serde_json::Value = serde_json::from_str(&payload).expect("event JSON");
        assert_eq!(value["response"]["id"], "resp_downstream");
        assert_eq!(
            value["response"]["output"][0]["internal_chat_message_metadata_passthrough"]["turn_id"],
            "turn_downstream"
        );
        let item_alias = value["response"]["output"][0]["id"]
            .as_str()
            .expect("item alias")
            .to_string();
        let call_alias = value["response"]["output"][0]["call_id"]
            .as_str()
            .expect("call alias")
            .to_string();
        assert_ne!(item_alias, "item_provider");
        assert_ne!(call_alias, "call_provider");

        let restored = store
            .edit(
                "namespace-sse",
                "acct_sse_translation",
                "scope-sse",
                move |editor| {
                    let mut request = serde_json::json!({
                        "previous_response_id":"resp_downstream",
                        "input":[{"type":"function_call_output","id":item_alias,"call_id":call_alias,"output":"ok"}]
                    })
                    .as_object()
                    .expect("request")
                    .clone();
                    translate_request_ids(editor, &mut request, &BTreeSet::new())?;
                    Ok(request)
                },
            )
            .await
            .expect("restore aliases");
        assert_eq!(restored["previous_response_id"], "resp_upstream");
        assert_eq!(restored["input"][0]["id"], "item_provider");
        assert_eq!(restored["input"][0]["call_id"], "call_provider");
    }
}
