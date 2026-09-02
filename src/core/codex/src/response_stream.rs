use crate::error::CoreFailure;
use crate::error::failure;
use crate::request_profile::UpstreamProfile;
use crate::response_headers::filtered_provider_headers;
use crate::response_sse_translation::UpstreamByteStream;
use crate::response_sse_translation::translated_sse_frames;
use crate::response_translation::ResponseStateContext;
use axum::body::Body;
use axum::http::HeaderMap;
use axum::http::HeaderName;
use axum::http::HeaderValue;
use axum::http::Response;
use futures_util::StreamExt;
use http_body::Frame;
use http_body_util::StreamBody;
use mini_sub2api_protocol_v1::CORE_TTFB_HEADER;
use mini_sub2api_protocol_v1::DELIVERY_STATE_TRAILER;
use mini_sub2api_protocol_v1::DeliveryState;
use mini_sub2api_protocol_v1::ErrorEnvelope;
use mini_sub2api_protocol_v1::FAILURE_PHASE_TRAILER;
use mini_sub2api_protocol_v1::FailureMetadata;
use mini_sub2api_protocol_v1::FailurePhase;
use mini_sub2api_protocol_v1::RETRY_ADVICE_TRAILER;
use mini_sub2api_protocol_v1::RetryAdvice;
use std::convert::Infallible;

const MAX_NON_STREAMING_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

pub(crate) async fn build_http_response(
    upstream: reqwest::Response,
    ttfb_ms: u128,
    downstream_expects_sse: bool,
    profile: UpstreamProfile,
    response_state: Option<ResponseStateContext>,
    gateway_request_id: &str,
) -> Result<Response<Body>, CoreFailure> {
    let filtered_headers = filtered_provider_headers(upstream.headers(), gateway_request_id)
        .map_err(|_| CoreFailure::UpstreamResponseFailed)?;
    if profile.uses_identity_state() && !upstream.status().is_success() {
        return normalized_upstream_failure(
            upstream.status(),
            ttfb_ms,
            filtered_headers,
            gateway_request_id,
            &CoreFailure::UpstreamResponseFailed,
        );
    }
    if profile.emulates_codex() && !downstream_expects_sse && upstream.status().is_success() {
        let diagnostic_headers = filtered_headers.clone();
        return match build_non_streaming_response(
            upstream,
            ttfb_ms,
            response_state.as_ref(),
            filtered_headers,
        )
        .await
        {
            Ok(response) => Ok(response),
            Err(error) => normalized_upstream_failure(
                error.status(),
                ttfb_ms,
                diagnostic_headers,
                gateway_request_id,
                &error,
            ),
        };
    }
    let diagnostic_headers = filtered_headers.clone();
    match build_streaming_response(
        upstream,
        ttfb_ms,
        downstream_expects_sse,
        response_state,
        filtered_headers,
    ) {
        Ok(response) => Ok(response),
        Err(error) => normalized_upstream_failure(
            error.status(),
            ttfb_ms,
            diagnostic_headers,
            gateway_request_id,
            &error,
        ),
    }
}

pub(crate) fn build_http_failure_response(
    upstream: reqwest::Response,
    ttfb_ms: u128,
    gateway_request_id: &str,
    failure: &CoreFailure,
) -> Result<Response<Body>, CoreFailure> {
    let filtered_headers = filtered_provider_headers(upstream.headers(), gateway_request_id)
        .map_err(|_| CoreFailure::UpstreamResponseFailed)?;
    normalized_upstream_failure(
        upstream.status(),
        ttfb_ms,
        filtered_headers,
        gateway_request_id,
        failure,
    )
}

fn build_streaming_response(
    upstream: reqwest::Response,
    ttfb_ms: u128,
    expects_sse: bool,
    response_state: Option<ResponseStateContext>,
    filtered_headers: HeaderMap,
) -> Result<Response<Body>, CoreFailure> {
    let status = upstream.status();
    let mut builder = Response::builder().status(status);
    let has_content_type = filtered_headers.contains_key(http::header::CONTENT_TYPE);
    for (name, value) in &filtered_headers {
        builder = builder.header(name, value);
    }
    if expects_sse && status.is_success() && !has_content_type {
        builder = builder.header(http::header::CONTENT_TYPE, "text/event-stream");
    }
    builder = builder.header(CORE_TTFB_HEADER, ttfb_ms.to_string());
    builder = builder.header(
        http::header::TRAILER,
        format!("{FAILURE_PHASE_TRAILER}, {DELIVERY_STATE_TRAILER}, {RETRY_ADVICE_TRAILER}"),
    );
    let translate_sse = response_state.is_some() && expects_sse && status.is_success();
    let upstream_stream: UpstreamByteStream = Box::pin(upstream.bytes_stream());
    if translate_sse {
        let stream = translated_sse_frames(
            upstream_stream,
            response_state.expect("translation context"),
            MAX_NON_STREAMING_RESPONSE_BYTES,
        );
        return builder
            .body(Body::new(StreamBody::new(stream)))
            .map_err(|_| CoreFailure::UpstreamResponseFailed);
    }
    let stream = futures_util::stream::unfold(
        (upstream_stream, false),
        |(mut upstream_stream, finished)| async move {
            if finished {
                return None;
            }
            match upstream_stream.next().await {
                Some(Ok(bytes)) => Some((
                    Ok::<Frame<bytes::Bytes>, Infallible>(Frame::data(bytes)),
                    (upstream_stream, false),
                )),
                Some(Err(_)) => {
                    let metadata = failure(
                        RetryAdvice::Never,
                        FailurePhase::UpstreamStream,
                        DeliveryState::Delivered,
                    );
                    Some((
                        Ok(Frame::trailers(failure_trailers(metadata))),
                        (upstream_stream, true),
                    ))
                }
                None => None,
            }
        },
    );
    builder
        .body(Body::new(StreamBody::new(stream)))
        .map_err(|_| CoreFailure::UpstreamResponseFailed)
}

async fn build_non_streaming_response(
    upstream: reqwest::Response,
    ttfb_ms: u128,
    response_state: Option<&ResponseStateContext>,
    mut filtered_headers: HeaderMap,
) -> Result<Response<Body>, CoreFailure> {
    let mut builder = Response::builder().status(upstream.status());
    filtered_headers.remove(http::header::CONTENT_TYPE);
    filtered_headers.remove(http::header::CONTENT_ENCODING);
    for (name, value) in &filtered_headers {
        builder = builder.header(name, value);
    }
    let mut bytes = Vec::new();
    let mut stream = upstream.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| CoreFailure::UpstreamResponseFailed)?;
        append_bounded(&mut bytes, &chunk, MAX_NON_STREAMING_RESPONSE_BYTES)?;
    }
    let terminal = terminal_response_from_sse(&bytes)?;
    let mut response = terminal.response;
    if let Some(state) = response_state {
        response = state
            .translate_terminal_value(response, terminal.kind == TerminalKind::Completed)
            .await
            .map_err(|_| CoreFailure::UpstreamResponseFailed)?;
    }
    let body = serde_json::to_vec(&response).map_err(|_| CoreFailure::UpstreamResponseFailed)?;
    builder
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(CORE_TTFB_HEADER, ttfb_ms.to_string())
        .body(Body::from(body))
        .map_err(|_| CoreFailure::UpstreamResponseFailed)
}

fn normalized_upstream_failure(
    status: axum::http::StatusCode,
    ttfb_ms: u128,
    mut filtered_headers: HeaderMap,
    gateway_request_id: &str,
    failure: &CoreFailure,
) -> Result<Response<Body>, CoreFailure> {
    filtered_headers.remove(http::header::CONTENT_TYPE);
    filtered_headers.remove(http::header::CONTENT_ENCODING);
    let envelope = ErrorEnvelope {
        error: mini_sub2api_protocol_v1::CoreError {
            code: failure.code().to_string(),
            message: failure.public_message().to_string(),
            request_id: gateway_request_id.to_string(),
            failure: failure.failure(),
        },
    };
    let body = serde_json::to_vec(&envelope).map_err(|_| CoreFailure::Internal)?;
    let mut builder = Response::builder().status(status);
    for (name, value) in &filtered_headers {
        builder = builder.header(name, value);
    }
    builder
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(CORE_TTFB_HEADER, ttfb_ms.to_string())
        .body(Body::from(body))
        .map_err(|_| CoreFailure::Internal)
}

fn append_bounded(
    destination: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: usize,
) -> Result<(), CoreFailure> {
    if destination
        .len()
        .checked_add(chunk.len())
        .is_none_or(|length| length > max_bytes)
    {
        return Err(CoreFailure::UpstreamResponseFailed);
    }
    destination.extend_from_slice(chunk);
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalKind {
    Completed,
    Failed,
    Incomplete,
}

struct TerminalResponse {
    kind: TerminalKind,
    response: serde_json::Value,
}

fn terminal_response_from_sse(bytes: &[u8]) -> Result<TerminalResponse, CoreFailure> {
    let text = std::str::from_utf8(bytes).map_err(|_| CoreFailure::UpstreamResponseFailed)?;
    let mut data = Vec::new();
    let mut terminal = None;
    for line in text.lines().chain(std::iter::once("")) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            if !data.is_empty() {
                let payload = data.join("\n");
                data.clear();
                if payload != "[DONE]" {
                    let event: serde_json::Value = serde_json::from_str(&payload)
                        .map_err(|_| CoreFailure::UpstreamResponseFailed)?;
                    let kind = match event.get("type").and_then(serde_json::Value::as_str) {
                        Some("response.completed") => Some(TerminalKind::Completed),
                        Some("response.failed") => Some(TerminalKind::Failed),
                        Some("response.incomplete") => Some(TerminalKind::Incomplete),
                        _ => None,
                    };
                    if let Some(kind) = kind
                        && let Some(response) = event.get("response").cloned()
                    {
                        terminal = Some(TerminalResponse { kind, response });
                    }
                }
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    terminal.ok_or(CoreFailure::UpstreamResponseFailed)
}

pub(crate) fn request_expects_sse(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("stream").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

fn failure_trailers(metadata: FailureMetadata) -> HeaderMap {
    let mut trailers = HeaderMap::new();
    trailers.insert(
        HeaderName::from_static("x-mini-sub2api-failure-phase"),
        HeaderValue::from_static(metadata.phase.as_str()),
    );
    trailers.insert(
        HeaderName::from_static("x-mini-sub2api-delivery-state"),
        HeaderValue::from_static(metadata.delivery_state.as_str()),
    );
    trailers.insert(
        HeaderName::from_static("x-mini-sub2api-retry-advice"),
        HeaderValue::from_static(metadata.retry_advice.as_str()),
    );
    trailers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_sse_response_is_extracted_across_standard_line_endings() {
        let body = b": keepalive\r\nevent: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\r\ndata: \"delta\":\"ok\"}\r\n\r\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"output\":[]}}\n\ndata: [DONE]\n\n";
        assert_eq!(
            terminal_response_from_sse(body)
                .expect("terminal response")
                .response,
            serde_json::json!({"id":"resp_test","output":[]})
        );
    }

    #[test]
    fn aggregated_sse_retains_each_terminal_kind() {
        for (event_type, expected) in [
            ("response.completed", TerminalKind::Completed),
            ("response.failed", TerminalKind::Failed),
            ("response.incomplete", TerminalKind::Incomplete),
        ] {
            let body = format!(
                "event: {event_type}\ndata: {{\"type\":\"{event_type}\",\"response\":{{\"id\":\"resp_terminal\"}}}}\n\n"
            );
            let terminal = terminal_response_from_sse(body.as_bytes()).expect("terminal response");
            assert_eq!(terminal.kind, expected);
            assert_eq!(terminal.response["id"], "resp_terminal");
        }
    }

    #[test]
    fn missing_terminal_sse_response_fails_closed() {
        let result = terminal_response_from_sse(
            b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
        );
        assert!(matches!(result, Err(CoreFailure::UpstreamResponseFailed)));
    }

    #[test]
    fn non_streaming_response_buffer_is_bounded() {
        let mut bytes = vec![1, 2, 3];
        append_bounded(&mut bytes, &[4], 4).expect("at limit");
        assert!(matches!(
            append_bounded(&mut bytes, &[5], 4),
            Err(CoreFailure::UpstreamResponseFailed)
        ));
        assert_eq!(bytes, vec![1, 2, 3, 4]);
    }
}
