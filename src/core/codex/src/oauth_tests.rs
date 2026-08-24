use super::*;
use pretty_assertions::assert_eq;

#[test]
fn pkce_is_url_safe_and_s256() {
    let pkce = generate_pkce();
    assert!(pkce.verifier.len() >= 43);
    assert!(!pkce.verifier.contains('='));
    let digest = Sha256::digest(pkce.verifier.as_bytes());
    let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    assert_eq!(pkce.challenge, expected);
}

#[test]
fn parses_account_and_expiration_from_test_jwt() {
    let claims = serde_json::json!({
        "exp": 1_900_000_000_i64,
        "https://api.openai.com/auth": {"chatgpt_account_id": "acct-upstream"}
    });
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&claims).expect("claims"));
    let token = format!("header.{payload}.signature");
    assert_eq!(
        account_id_from_token(&token).as_deref(),
        Some("acct-upstream")
    );
    assert_eq!(
        token_expiration(&token).expect("expiration"),
        DateTime::from_timestamp(1_900_000_000, 0)
    );
}

#[test]
fn identifies_permanent_refresh_errors() {
    assert!(is_permanent_refresh_error(
        StatusCode::BAD_REQUEST,
        r#"{"error":{"code":"refresh_token_reused"}}"#
    ));
    assert!(is_permanent_refresh_error(
        StatusCode::BAD_REQUEST,
        r#"{"error":"invalid_grant"}"#
    ));
    assert!(!is_permanent_refresh_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        r#"{"error":"invalid_grant"}"#
    ));
    assert!(!is_permanent_refresh_error(
        StatusCode::BAD_REQUEST,
        r#"{"error":{"code":"server_error"}}"#
    ));
    assert!(!is_permanent_refresh_error(
        StatusCode::BAD_REQUEST,
        r#"{"message":"refresh_token_reused appeared only in text"}"#
    ));
}

#[test]
fn refresh_and_revoke_requests_use_codex_identity_headers() {
    let request = codex_auth_request(Client::new().post("http://127.0.0.1/oauth/token"))
        .build()
        .expect("request");
    assert_eq!(
        request
            .headers()
            .get("originator")
            .and_then(|value| value.to_str().ok()),
        Some(crate::upstream_request::DEFAULT_CODEX_ORIGINATOR)
    );
    assert!(
        request
            .headers()
            .get(http::header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("codex_cli_rs/0.149.0 ("))
    );
}
