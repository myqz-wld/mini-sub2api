use reqwest::ClientBuilder;
use url::Url;

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
            reqwest::Client::builder()
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
}
