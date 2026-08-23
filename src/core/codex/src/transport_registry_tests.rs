use super::*;
use crate::test_support::spawn_loopback;
use crate::upstream_request::ResolvedAuth;
use crate::upstream_request::build_websocket;
use crate::websocket_connector::WebSocketHandshake;
use axum::Router;
use axum::extract::ConnectInfo;
use axum::extract::State;
use axum::extract::ws::WebSocketUpgrade;
use axum::routing::get;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

#[test]
fn registry_reuses_one_context_per_account_and_policy_revision() {
    let registry = TransportRegistry::new().expect("registry");
    let policy = CredentialTransportPolicy::default();
    let first = registry.context("acct_first", policy).expect("first");
    let repeated = registry.context("acct_first", policy).expect("repeated");
    let second = registry.context("acct_second", policy).expect("second");
    let revised = registry
        .context(
            "acct_first",
            CredentialTransportPolicy { egress_revision: 1 },
        )
        .expect("revised");

    assert!(Arc::ptr_eq(&first, &repeated));
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(!Arc::ptr_eq(&first, &revised));
    assert!(
        first
            .websocket
            .shares_tls_state_with(&first.direct_websocket)
    );
    assert!(!first.websocket.shares_tls_state_with(&second.websocket));
    assert!(!first.websocket.shares_tls_state_with(&revised.websocket));
}

#[test]
fn concurrent_first_access_builds_exactly_one_context() {
    let registry = Arc::new(TransportRegistry::new().expect("registry"));
    let mut workers = Vec::new();
    for _ in 0..12 {
        let registry = Arc::clone(&registry);
        workers.push(std::thread::spawn(move || {
            registry
                .context("acct_concurrent", CredentialTransportPolicy::default())
                .expect("context")
        }));
    }
    let contexts = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker"))
        .collect::<Vec<_>>();
    assert!(
        contexts[1..]
            .iter()
            .all(|context| Arc::ptr_eq(&contexts[0], context))
    );
}

#[test]
fn websocket_tls_uses_aws_lc_native_roots_without_alpn_and_prefers_pq() {
    let (roots, native_root_count) = load_websocket_roots().expect("native roots");
    let config = build_websocket_tls_config(roots).expect("TLS config");
    assert!(native_root_count > 0);
    assert!(config.alpn_protocols.is_empty());
    let provider = rustls::crypto::CryptoProvider::get_default().expect("installed provider");
    assert!(
        provider
            .signature_verification_algorithms
            .supported_schemes()
            .contains(&REQUIRED_AWS_LC_SIGNATURE_SCHEME)
    );
    assert_eq!(
        provider.kx_groups.first().map(|group| group.name()),
        Some(rustls::NamedGroup::X25519MLKEM768)
    );
}

#[derive(Clone, Default)]
struct PeerCapture(Arc<AsyncMutex<Vec<SocketAddr>>>);

#[tokio::test]
async fn http_pool_reuses_within_account_and_not_across_accounts() {
    async fn capture_peer(
        ConnectInfo(peer): ConnectInfo<SocketAddr>,
        State(capture): State<PeerCapture>,
    ) -> &'static str {
        capture.0.lock().await.push(peer);
        "ok"
    }

    let capture = PeerCapture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let app = Router::new()
        .route("/", get(capture_peer))
        .with_state(capture.clone());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("server");
    });
    let url = format!("http://{address}/");
    crate::test_support::assert_loopback_url(&url);

    let registry = TransportRegistry::new().expect("registry");
    let first = registry
        .context("acct_pool_first", CredentialTransportPolicy::default())
        .expect("first context");
    let second = registry
        .context("acct_pool_second", CredentialTransportPolicy::default())
        .expect("second context");
    for _ in 0..2 {
        let response = first
            .http_client_for_url(&url)
            .get(&url)
            .send()
            .await
            .expect("same-account request");
        assert_eq!(response.text().await.expect("body"), "ok");
    }
    let response = second
        .http_client_for_url(&url)
        .get(&url)
        .send()
        .await
        .expect("other-account request");
    assert_eq!(response.text().await.expect("body"), "ok");

    let peers = capture.0.lock().await.clone();
    assert_eq!(peers.len(), 3);
    assert_eq!(peers[0], peers[1]);
    assert_ne!(peers[0], peers[2]);
    task.abort();
}

#[tokio::test]
async fn literal_loopback_http_and_websocket_bypass_bad_proxy() {
    let websocket_app = Router::new().route(
        "/responses",
        get(|upgrade: WebSocketUpgrade| async move {
            upgrade.on_upgrade(|socket| async move {
                drop(socket);
            })
        }),
    );
    let websocket_server = spawn_loopback(websocket_app).await;
    let http_server = spawn_loopback(Router::new().route("/", get(|| async { "direct" }))).await;
    let registry = TransportRegistry::new_with_proxy_url("http://127.0.0.1:1").expect("registry");
    let context = registry
        .context("acct_direct", CredentialTransportPolicy::default())
        .expect("context");

    let response = context
        .http_client_for_url(&http_server.base_url)
        .get(&http_server.base_url)
        .send()
        .await
        .expect("direct HTTP");
    assert_eq!(response.text().await.expect("HTTP body"), "direct");

    let websocket_url = format!("{}/responses", websocket_server.base_url);
    let (request, config) = build_websocket(
        &http::HeaderMap::new(),
        &websocket_url,
        &ResolvedAuth::OpenAiApiKey {
            token: "offline-test-key".to_string(),
        },
        crate::responses_websocket::MAX_WEBSOCKET_MESSAGE_BYTES,
    )
    .expect("WebSocket request");
    let handshake = context
        .websocket_connector_for_url(&websocket_url)
        .connect(request, config)
        .await
        .expect("direct WebSocket");
    assert_eq!(handshake.status(), http::StatusCode::SWITCHING_PROTOCOLS);
    let WebSocketHandshake::Connected { socket, .. } = handshake else {
        panic!("expected WebSocket connection");
    };
    drop(socket);
}

#[tokio::test]
async fn clients_do_not_follow_redirects() {
    let server = spawn_loopback(Router::new().route(
        "/",
        get(|| async { axum::response::Redirect::temporary("/unexpected") }),
    ))
    .await;
    let registry = TransportRegistry::new().expect("registry");
    let context = registry
        .context("acct_redirect", CredentialTransportPolicy::default())
        .expect("context");
    let response = context
        .http_client_for_url(&server.base_url)
        .get(&server.base_url)
        .send()
        .await
        .expect("redirect response");
    assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
}
