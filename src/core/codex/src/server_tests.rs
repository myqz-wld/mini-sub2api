use super::*;
use pretty_assertions::assert_eq;

#[test]
fn internal_auth_is_constant_time_hash_checked() {
    let expected: [u8; 32] = Sha256::digest(b"internal-secret-value-that-is-long").into();
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer internal-secret-value-that-is-long"),
    );
    assert!(validate_internal_auth(&headers, &expected).is_ok());
    headers.insert(
        http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer wrong-secret-value-that-is-long"),
    );
    assert!(validate_internal_auth(&headers, &expected).is_err());
}

#[test]
fn only_loopback_internal_listeners_are_valid() {
    assert_eq!(
        parse_internal_listen("127.0.0.1:0").expect("IPv4 loopback"),
        "127.0.0.1:0".parse().expect("address")
    );
    assert!(parse_internal_listen("0.0.0.0:8080").is_err());
    assert!(parse_internal_listen("192.168.1.2:8080").is_err());
}

#[test]
fn forwarding_drops_credentials_and_unreviewed_headers() {
    let mut source = HeaderMap::new();
    source.insert(
        http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer downstream"),
    );
    source.insert("x-codex-turn-state", HeaderValue::from_static("sticky"));
    for (name, value) in [
        ("content-encoding", "zstd"),
        ("originator", "codex_exec"),
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
        ("x-openai-internal-codex-responses-lite", "true"),
    ] {
        source.insert(
            HeaderName::from_static(name),
            HeaderValue::from_str(value).expect("header value"),
        );
    }
    source.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.1"));
    let got = forwarded_headers(&source);
    assert_eq!(
        got.get("x-codex-turn-state").and_then(|v| v.to_str().ok()),
        Some("sticky")
    );
    for (name, expected) in [
        ("content-encoding", "zstd"),
        ("originator", "codex_exec"),
        ("session-id", "session-test"),
        ("thread-id", "thread-test"),
        ("x-openai-internal-codex-responses-lite", "true"),
    ] {
        assert_eq!(
            got.get(name).and_then(|value| value.to_str().ok()),
            Some(expected)
        );
    }
    assert!(!got.contains_key(http::header::AUTHORIZATION));
    assert!(!got.contains_key("x-forwarded-for"));
}
