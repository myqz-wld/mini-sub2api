use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("x-codex-guardian", "classifier"),
        ("session-id", "synthetic-session"),
        ("thread-id", "synthetic-thread"),
        ("x-openai-subagent", "guardian"),
        ("x-codex-window-id", "synthetic-thread:0"),
        ("x-client-request-id", "synthetic-thread"),
        ("x-openai-internal-codex-responses-lite", "true"),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    headers
}

fn auth() -> ResolvedAuth {
    ResolvedAuth::CodexOAuth {
        token: "synthetic-only".into(),
        account_id: "synthetic-account".into(),
    }
}

#[tokio::test]
async fn classifier_http_peer_observes_uncompressed_source_order() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/responses", listener.local_addr().unwrap());
    crate::test_support::assert_loopback_url(&url);
    let client = Client::builder().no_proxy().build().unwrap();
    let request = build(
        &client,
        &headers(),
        &url,
        &auth(),
        UpstreamProfile::CodexSubscription1560,
        Bytes::from_static(b"{}"),
    )
    .unwrap();
    let send = tokio::spawn(async move { client.execute(request).await.unwrap() });
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut bytes = Vec::new();
    let end = loop {
        let mut chunk = [0; 2048];
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
        assert!(bytes.len() < 16384);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n")
            && bytes.len() >= end + 6
        {
            break end;
        }
    };
    let head = std::str::from_utf8(&bytes[..end]).unwrap();
    let names: Vec<_> = head
        .lines()
        .skip(1)
        .map(|line| line.split_once(':').unwrap().0)
        .collect();
    // Official release capture: role/session headers -> request/auth -> client defaults.
    assert_eq!(
        names,
        [
            "version",
            "session-id",
            "thread-id",
            "x-codex-guardian",
            "x-openai-subagent",
            "x-codex-window-id",
            "x-openai-internal-codex-responses-lite",
            "x-client-request-id",
            "accept",
            "content-type",
            "authorization",
            "chatgpt-account-id",
            "originator",
            "user-agent",
            "host",
            "content-length"
        ]
    );
    assert_eq!(&bytes[end + 4..], b"{}");
    socket
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    assert!(send.await.unwrap().status().is_success());
}

#[test]
fn classifier_websocket_serialization_preserves_lite_in_its_source_merge_order() {
    let (request, config) = build_websocket(
        &headers(),
        "http://127.0.0.1:1/responses",
        &auth(),
        UpstreamProfile::CodexSubscription1560,
        4096,
    )
    .unwrap();
    let (bytes, _) = tokio_tungstenite::tungstenite::handshake::client::generate_request(
        request,
        Some(&config.extensions),
    )
    .unwrap();
    let raw = String::from_utf8(bytes).unwrap();
    let names: Vec<_> = raw
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':').map(|(name, _)| name))
        .collect();
    assert_eq!(
        names,
        [
            "Host",
            "Connection",
            "Upgrade",
            "Sec-WebSocket-Version",
            "Sec-WebSocket-Key",
            "chatgpt-account-id",
            "authorization",
            "user-agent",
            "originator",
            "openai-beta",
            "version",
            "session-id",
            "thread-id",
            "x-codex-guardian",
            "x-openai-subagent",
            "x-codex-window-id",
            "x-openai-internal-codex-responses-lite",
            "x-client-request-id",
            "sec-websocket-extensions"
        ]
    );
    assert!(raw.contains("x-openai-internal-codex-responses-lite: true\r\n"));
}
