use reqwest::ClientBuilder;
use url::Url;

/// Start every provider-facing HTTP client on reqwest's native TLS backend.
///
/// The workspace also enables rustls for the separately configured provider WebSocket path, so
/// selecting native TLS explicitly prevents feature unification from changing HTTP behavior.
pub fn native_tls_builder() -> ClientBuilder {
    reqwest::Client::builder().use_native_tls()
}

pub fn apply_loopback_proxy_policy(builder: ClientBuilder, raw_url: &str) -> ClientBuilder {
    if has_literal_loopback_host(raw_url) {
        builder.no_proxy()
    } else {
        builder
    }
}

pub fn has_literal_loopback_host(raw_url: &str) -> bool {
    let Ok(url) = Url::parse(raw_url) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(_)) | None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::spawn_loopback;
    use axum::Router;
    use axum::routing::get;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    #[test]
    fn identifies_only_literal_loopback_hosts() {
        assert!(has_literal_loopback_host("http://127.0.0.1:1234/path"));
        assert!(has_literal_loopback_host("https://[::1]:1234/path"));
        assert!(!has_literal_loopback_host("http://localhost:1234/path"));
        assert!(!has_literal_loopback_host("https://192.168.1.8/path"));
    }

    #[tokio::test]
    async fn loopback_policy_overrides_an_explicit_bad_proxy() {
        let mock = spawn_loopback(Router::new().route("/", get(|| async { "direct" }))).await;
        let client = apply_loopback_proxy_policy(
            native_tls_builder()
                .proxy(reqwest::Proxy::all("http://127.0.0.1:1").expect("explicit test proxy")),
            &mock.base_url,
        )
        .build()
        .expect("direct client");
        let response = client
            .get(&mock.base_url)
            .send()
            .await
            .expect("loopback request bypasses proxy");
        assert_eq!(response.text().await.expect("body"), "direct");
    }

    #[tokio::test]
    async fn native_tls_builder_emits_a_client_hello_only_to_loopback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback TLS listener");
        let address = listener.local_addr().expect("loopback TLS address");
        let url = format!("https://{address}/native-tls-probe");
        crate::test_support::assert_loopback_url(&url);

        let capture = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("TLS connection");
            let mut header = [0_u8; 5];
            stream
                .read_exact(&mut header)
                .await
                .expect("TLS record header");
            assert_eq!(header[0], 22, "expected TLS handshake record");
            let record_len = usize::from(u16::from_be_bytes([header[3], header[4]]));
            assert!((4..=64 * 1024).contains(&record_len), "TLS record length");
            let mut payload = vec![0_u8; record_len];
            stream
                .read_exact(&mut payload)
                .await
                .expect("TLS record payload");
            payload
        });

        let client = native_tls_builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .build()
            .expect("native TLS client");
        let error = tokio::time::timeout(Duration::from_secs(2), client.get(url).send())
            .await
            .expect("native TLS request timeout")
            .expect_err("capture server closes before TLS completes");
        assert!(error.is_connect(), "unexpected native TLS error: {error}");

        let payload = capture.await.expect("ClientHello capture");
        assert_eq!(payload[0], 1, "expected ClientHello handshake");
    }
}
