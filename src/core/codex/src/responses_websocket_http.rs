use crate::websocket_connector::WebSocketHandshake;
use axum::body::Body;
use axum::http::HeaderMap;
use axum::http::Response;
use bytes::Bytes;

use crate::response_headers::filtered_provider_headers;

const MAX_HANDSHAKE_REJECTION_BYTES: usize = 64 * 1024;

pub(crate) async fn rejection_response(
    handshake: WebSocketHandshake,
    gateway_request_id: &str,
) -> Response<Body> {
    let WebSocketHandshake::Rejected(upstream) = handshake else {
        return Response::new(Body::empty());
    };
    let status = upstream.status();
    let headers = filtered_provider_headers(upstream.headers(), gateway_request_id)
        .unwrap_or_else(|_| HeaderMap::new());
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

pub(crate) fn filtered_upgrade_headers(
    source: &HeaderMap,
    gateway_request_id: &str,
) -> Result<HeaderMap, ()> {
    filtered_provider_headers(source, gateway_request_id)
}

pub(crate) fn copy_headers(destination: &mut HeaderMap, source: &HeaderMap) {
    for (name, value) in source {
        destination.append(name.clone(), value.clone());
    }
}
