use crate::error::CoreFailure;
use crate::error::failure;
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
use mini_sub2api_protocol_v1::FAILURE_PHASE_TRAILER;
use mini_sub2api_protocol_v1::FailureMetadata;
use mini_sub2api_protocol_v1::FailurePhase;
use mini_sub2api_protocol_v1::RETRY_ADVICE_TRAILER;
use mini_sub2api_protocol_v1::RetryAdvice;
use std::collections::HashSet;
use std::convert::Infallible;

pub(crate) fn build_streaming_response(
    upstream: reqwest::Response,
    ttfb_ms: u128,
    expects_sse: bool,
) -> Result<Response<Body>, CoreFailure> {
    let status = upstream.status();
    let mut builder = Response::builder().status(status);
    let connection_headers = nominated_connection_headers(upstream.headers());
    let has_content_type = upstream.headers().contains_key(http::header::CONTENT_TYPE);
    for (name, value) in upstream.headers() {
        if is_safe_response_header(name) && !connection_headers.contains(name.as_str()) {
            builder = builder.header(name, value);
        }
    }
    if expects_sse && status.is_success() && !has_content_type {
        builder = builder.header(http::header::CONTENT_TYPE, "text/event-stream");
    }
    builder = builder.header(CORE_TTFB_HEADER, ttfb_ms.to_string());
    builder = builder.header(
        http::header::TRAILER,
        format!("{FAILURE_PHASE_TRAILER}, {DELIVERY_STATE_TRAILER}, {RETRY_ADVICE_TRAILER}"),
    );
    let upstream_stream = Box::pin(upstream.bytes_stream());
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

fn nominated_connection_headers(headers: &HeaderMap) -> HashSet<String> {
    headers
        .get_all(http::header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn is_safe_response_header(name: &HeaderName) -> bool {
    if name.as_str().starts_with("x-mini-sub2api-") {
        return false;
    }
    !matches!(
        name.as_str(),
        "connection"
            | "content-length"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "set-cookie"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}
