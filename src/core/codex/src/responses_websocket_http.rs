use crate::websocket_connector::WebSocketHandshake;
use axum::body::Body;
use axum::http::HeaderMap;
use axum::http::HeaderName;
use axum::http::Response;
use bytes::Bytes;

const MAX_HANDSHAKE_REJECTION_BYTES: usize = 64 * 1024;

const SAFE_UPGRADE_RESPONSE_HEADERS: &[&str] = &[
    "openai-model",
    "x-codex-turn-state",
    "x-models-etag",
    "x-reasoning-included",
    "x-request-id",
];

const SAFE_REJECTION_RESPONSE_HEADERS: &[&str] = &["content-type", "retry-after", "x-request-id"];

pub(crate) async fn rejection_response(handshake: WebSocketHandshake) -> Response<Body> {
    let WebSocketHandshake::Rejected(upstream) = handshake else {
        return Response::new(Body::empty());
    };
    let status = upstream.status();
    let headers = filtered_headers(upstream.headers(), SAFE_REJECTION_RESPONSE_HEADERS);
    let preserve_body = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let value = value.to_ascii_lowercase();
            value.starts_with("application/json") || value.starts_with("text/")
        });
    let body = if preserve_body {
        upstream
            .into_body()
            .filter(|body| body.len() <= MAX_HANDSHAKE_REJECTION_BYTES)
            .map(Bytes::from)
            .unwrap_or_default()
    } else {
        Bytes::new()
    };
    let mut response = Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()));
    copy_headers(response.headers_mut(), &headers);
    response
}

pub(crate) fn filtered_upgrade_headers(source: &HeaderMap) -> HeaderMap {
    filtered_headers(source, SAFE_UPGRADE_RESPONSE_HEADERS)
}

fn filtered_headers(source: &HeaderMap, allowed: &[&'static str]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for name in allowed {
        let name = HeaderName::from_static(name);
        for value in source.get_all(&name) {
            headers.append(name.clone(), value.clone());
        }
    }
    headers
}

pub(crate) fn copy_headers(destination: &mut HeaderMap, source: &HeaderMap) {
    for (name, value) in source {
        destination.append(name.clone(), value.clone());
    }
}
