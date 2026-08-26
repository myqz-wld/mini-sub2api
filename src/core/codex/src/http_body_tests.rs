use super::*;
use http::HeaderValue;

#[test]
fn absent_or_identity_encoding_returns_the_uncompressed_body() {
    let original = Bytes::from_static(br#" {"future":true} "#);
    let mut absent = HeaderMap::new();
    assert_eq!(
        decode_emulated_request_body(&mut absent, original.clone(), original.len()),
        Ok(original.clone())
    );

    let mut identity = HeaderMap::new();
    identity.insert(
        http::header::CONTENT_ENCODING,
        HeaderValue::from_static(" identity "),
    );
    assert_eq!(
        decode_emulated_request_body(&mut identity, original.clone(), original.len()),
        Ok(original)
    );
    assert!(!identity.contains_key(http::header::CONTENT_ENCODING));
}

#[test]
fn zstd_is_decoded_within_the_limit_and_removed_from_headers() {
    let original = br#"{"model":"gpt-5.6-sol"}"#;
    let compressed = zstd::stream::encode_all(Cursor::new(original), 3).expect("compress fixture");
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::CONTENT_ENCODING,
        HeaderValue::from_static("ZsTd"),
    );

    let decoded =
        decode_emulated_request_body(&mut headers, Bytes::from(compressed), original.len())
            .expect("decode request");

    assert_eq!(decoded, Bytes::from_static(original));
    assert!(!headers.contains_key(http::header::CONTENT_ENCODING));
}

#[test]
fn zstd_expansion_past_the_limit_is_rejected() {
    let original = vec![b'x'; 4096];
    let compressed = zstd::stream::encode_all(Cursor::new(&original), 3).expect("compress fixture");
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::CONTENT_ENCODING,
        HeaderValue::from_static("zstd"),
    );

    assert!(decode_emulated_request_body(&mut headers, Bytes::from(compressed), 1024).is_err());
}

#[test]
fn unknown_multiple_or_malformed_encoding_is_rejected() {
    for encoding in ["gzip", "identity, zstd", ""] {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_str(encoding).expect("encoding header"),
        );
        assert!(
            decode_emulated_request_body(&mut headers, Bytes::from_static(b"body"), 16).is_err()
        );
    }

    let mut repeated = HeaderMap::new();
    repeated.append(
        http::header::CONTENT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    repeated.append(
        http::header::CONTENT_ENCODING,
        HeaderValue::from_static("zstd"),
    );
    assert!(decode_emulated_request_body(&mut repeated, Bytes::from_static(b"body"), 16).is_err());
}

#[test]
fn corrupt_zstd_is_rejected() {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::CONTENT_ENCODING,
        HeaderValue::from_static("zstd"),
    );

    assert!(
        decode_emulated_request_body(&mut headers, Bytes::from_static(b"not-zstd"), 1024).is_err()
    );
}
