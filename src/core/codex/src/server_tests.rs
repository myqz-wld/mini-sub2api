use super::*;
use http::HeaderValue;
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
