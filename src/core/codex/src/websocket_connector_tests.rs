use super::*;
use crate::request_profile::UpstreamProfile;
use crate::transport_registry::CredentialTransportPolicy;
use crate::transport_registry::TransportRegistry;
use crate::upstream_request::ResolvedAuth;
use crate::upstream_request::build_websocket;
use futures_util::SinkExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::accept_async_with_config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::extensions::ExtensionsConfig;
use tokio_tungstenite::tungstenite::extensions::compression::deflate::DeflateConfig;

#[test]
fn proxy_lookup_maps_websocket_schemes_without_changing_authority_or_path() {
    let ws: Uri = "ws://127.0.0.1:9000/v1/responses?trace=1"
        .parse()
        .expect("WS URI");
    let wss: Uri = "wss://example.test/v1/responses".parse().expect("WSS URI");

    assert_eq!(
        proxy_lookup_uri(&ws)
            .expect("HTTP proxy lookup")
            .to_string(),
        "http://127.0.0.1:9000/v1/responses?trace=1"
    );
    assert_eq!(
        proxy_lookup_uri(&wss)
            .expect("HTTPS proxy lookup")
            .to_string(),
        "https://example.test/v1/responses"
    );
}

#[tokio::test]
async fn production_connector_negotiates_and_uses_permessage_deflate_on_loopback() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener");
    let address = listener.local_addr().expect("loopback address");
    let upstream_url = format!("http://{address}/v1/responses");
    crate::test_support::assert_loopback_url(&upstream_url);

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("loopback connection");
        let mut extensions = ExtensionsConfig::default();
        extensions.permessage_deflate = Some(DeflateConfig::default());
        let mut config = WebSocketConfig::default();
        config.extensions = extensions;
        let mut websocket = accept_async_with_config(stream, Some(config))
            .await
            .expect("deflate handshake");
        let message = websocket
            .next()
            .await
            .expect("client message")
            .expect("valid client message");
        let Message::Text(text) = message else {
            panic!("expected text frame");
        };
        assert_eq!(text.len(), 32 * 1024);
        websocket
            .send(Message::Text("ack".into()))
            .await
            .expect("server response");
    });

    let registry = TransportRegistry::new().expect("transport registry");
    let context = registry
        .context("acct_deflate", CredentialTransportPolicy::default())
        .expect("credential context");
    let (request, config) = build_websocket(
        &http::HeaderMap::new(),
        &upstream_url,
        &ResolvedAuth::OpenAiApiKey {
            token: "offline-deflate-key-not-real".to_string(),
        },
        UpstreamProfile::BareOpenAi,
        1024 * 1024,
    )
    .expect("WebSocket request");
    let handshake = context
        .websocket_connector_for_url(&upstream_url)
        .connect(request, config)
        .await
        .expect("WebSocket handshake");
    assert_eq!(
        handshake
            .headers()
            .get("sec-websocket-extensions")
            .and_then(|value| value.to_str().ok()),
        Some("permessage-deflate")
    );
    let WebSocketHandshake::Connected { socket, .. } = handshake else {
        panic!("expected connected WebSocket");
    };
    let mut socket = *socket;
    socket
        .send(Message::Text("a".repeat(32 * 1024).into()))
        .await
        .expect("compressed client frame");
    let reply = socket
        .next()
        .await
        .expect("server reply")
        .expect("valid server reply");
    assert_eq!(reply, Message::Text("ack".into()));
    server.await.expect("loopback server");
}

#[tokio::test]
async fn explicit_http_proxy_tunnels_without_resolving_the_target_host() {
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("upstream listener");
    let upstream_address = upstream_listener.local_addr().expect("upstream address");
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("proxy listener");
    let proxy_address = proxy_listener.local_addr().expect("proxy address");

    let upstream = tokio::spawn(async move {
        let (stream, _) = upstream_listener
            .accept()
            .await
            .expect("upstream connection");
        let mut extensions = ExtensionsConfig::default();
        extensions.permessage_deflate = Some(DeflateConfig::default());
        let mut config = WebSocketConfig::default();
        config.extensions = extensions;
        let mut websocket = accept_async_with_config(stream, Some(config))
            .await
            .expect("upstream WebSocket");
        let message = websocket
            .next()
            .await
            .expect("proxied message")
            .expect("valid proxied message");
        assert_eq!(message, Message::Text("through-proxy".into()));
        websocket
            .send(Message::Text("ack".into()))
            .await
            .expect("upstream response");
    });
    let proxy = tokio::spawn(async move {
        let (mut client, _) = proxy_listener.accept().await.expect("proxy connection");
        let mut request = Vec::new();
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let mut chunk = [0_u8; 1024];
            let count = client.read(&mut chunk).await.expect("CONNECT request");
            assert!(count > 0, "CONNECT request closed early");
            request.extend_from_slice(&chunk[..count]);
            assert!(request.len() < 8192, "CONNECT request too large");
        }
        let request = std::str::from_utf8(&request).expect("ASCII CONNECT request");
        assert!(request.starts_with(&format!(
            "CONNECT example.test:{} HTTP/1.1\r\n",
            upstream_address.port()
        )));
        let mut target = tokio::net::TcpStream::connect(upstream_address)
            .await
            .expect("proxy target");
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .expect("CONNECT response");
        tokio::io::copy_bidirectional(&mut client, &mut target)
            .await
            .expect("proxy tunnel");
    });

    let registry = TransportRegistry::new_with_proxy_url(&format!("http://{proxy_address}"))
        .expect("proxied registry");
    let context = registry
        .context("acct_proxy", CredentialTransportPolicy::default())
        .expect("credential context");
    let upstream_url = format!("http://example.test:{}/responses", upstream_address.port());
    let (request, config) = build_websocket(
        &http::HeaderMap::new(),
        &upstream_url,
        &ResolvedAuth::OpenAiApiKey {
            token: "offline-proxy-key-not-real".to_string(),
        },
        UpstreamProfile::BareOpenAi,
        1024 * 1024,
    )
    .expect("proxied request");
    let handshake = context
        .websocket_connector_for_url(&upstream_url)
        .connect(request, config)
        .await
        .expect("proxied handshake");
    let WebSocketHandshake::Connected { socket, .. } = handshake else {
        panic!("expected proxied WebSocket");
    };
    let mut socket = *socket;
    socket
        .send(Message::Text("through-proxy".into()))
        .await
        .expect("proxied send");
    assert_eq!(
        socket
            .next()
            .await
            .expect("proxied reply")
            .expect("valid proxied reply"),
        Message::Text("ack".into())
    );
    drop(socket);
    upstream.await.expect("upstream task");
    proxy.await.expect("proxy task");
}
