use super::*;
use crate::test_support::spawn_loopback;
use crate::test_support::test_jwt;
use axum::Json;
use axum::Router;
use axum::routing::post;
use std::collections::HashMap;

#[tokio::test]
async fn device_flow_uses_loopback_mock_and_persists_tokens() {
    let account_id = "chatgpt-account-test";
    let id_token = test_jwt(Some(account_id), 3600);
    let access_token = test_jwt(None, 3600);
    let app = Router::new()
        .route(
            "/api/accounts/deviceauth/usercode",
            post(|| async {
                Json(serde_json::json!({
                    "device_auth_id": "device-test",
                    "user_code": "TEST-CODE",
                    "interval": "0"
                }))
            }),
        )
        .route(
            "/api/accounts/deviceauth/token",
            post(|| async {
                Json(serde_json::json!({
                    "authorization_code": "authorization-test",
                    "code_challenge": "challenge-test",
                    "code_verifier": "verifier-test"
                }))
            }),
        )
        .route(
            "/oauth/token",
            post({
                let id_token = id_token.clone();
                let access_token = access_token.clone();
                move |headers: http::HeaderMap, body: String| {
                    let id_token = id_token.clone();
                    let access_token = access_token.clone();
                    async move {
                        assert_eq!(
                            headers[http::header::CONTENT_TYPE],
                            "application/x-www-form-urlencoded"
                        );
                        let fields = body.split('&').collect::<Vec<_>>();
                        assert_eq!(
                            fields
                                .iter()
                                .map(|f| f.split('=').next().unwrap())
                                .collect::<Vec<_>>(),
                            [
                                "grant_type",
                                "client_id",
                                "code",
                                "redirect_uri",
                                "code_verifier"
                            ]
                        );
                        assert_eq!(fields[0], "grant_type=authorization_code");
                        assert_eq!(fields[1], "client_id=client-test");
                        assert_eq!(fields[2], "code=authorization-test");
                        assert!(fields[3].starts_with("redirect_uri=http%3A%2F%2F127.0.0.1%3A"));
                        assert_eq!(fields[4], "code_verifier=verifier-test");
                        Json(serde_json::json!({
                            "id_token": id_token,
                            "access_token": access_token,
                            "refresh_token": "refresh-test"
                        }))
                    }
                }
            }),
        );
    let mock = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");

    let metadata = login(
        &vault,
        LoginFlow::Device,
        OAuthConfig {
            issuer: mock.base_url.clone(),
            client_id: "client-test".to_string(),
            upstream_url: format!("{}/responses", mock.base_url),
            fingerprint_mode: crate::fingerprint::FingerprintMode::Device,
        },
    )
    .await
    .expect("device login");

    assert_eq!(metadata.auth_kind, "codex_oauth");
    assert_eq!(metadata.upstream_account_id.as_deref(), Some(account_id));
    let locked = vault
        .lock_record(&metadata.account_ref)
        .await
        .expect("stored record");
    match &locked.record.material {
        CredentialMaterial::CodexOAuth {
            refresh_token,
            issuer,
            ..
        } => {
            assert_eq!(refresh_token, "refresh-test");
            assert_eq!(issuer, &mock.base_url);
        }
        CredentialMaterial::OpenAiApiKey { .. } => panic!("wrong credential kind"),
    }
}

#[test]
fn rejects_non_http_auth_urls_before_network_access() {
    assert!(validate_auth_url("file:///tmp/not-network").is_err());
    assert!(validate_auth_url("not a URL").is_err());
    assert!(validate_auth_url("http://192.168.1.8/oauth").is_err());
    assert!(validate_auth_url("http://127.0.0.1:1234/oauth").is_ok());
}

#[test]
fn browser_authorize_url_uses_codex_originator_and_registered_ports() {
    let config = OAuthConfig {
        issuer: "https://auth.openai.com".to_string(),
        client_id: "client-test".to_string(),
        upstream_url: "https://chatgpt.com/backend-api/codex/responses".to_string(),
        fingerprint_mode: crate::fingerprint::FingerprintMode::Device,
    };
    let pkce = Pkce {
        verifier: "verifier".to_string(),
        challenge: "challenge".to_string(),
    };
    let url = authorize_url(
        &config,
        &format!("http://127.0.0.1:{DEFAULT_CALLBACK_PORT}/auth/callback"),
        &pkce,
        "state-test",
    )
    .expect("authorize URL");
    let pairs = url.query_pairs().into_owned().collect::<HashMap<_, _>>();

    assert_eq!(
        pairs.get("originator").map(String::as_str),
        Some(crate::upstream_request::DEFAULT_CODEX_ORIGINATOR)
    );
    assert_eq!(
        pairs.get("redirect_uri").map(String::as_str),
        Some("http://127.0.0.1:1455/auth/callback")
    );
    assert!(url.as_str().contains("scope=openid+profile+email"));
    assert_eq!(
        url.query_pairs()
            .map(|(key, _)| key.into_owned())
            .collect::<Vec<_>>(),
        [
            "response_type",
            "client_id",
            "redirect_uri",
            "code_challenge",
            "code_challenge_method",
            "state",
            "scope",
            "id_token_add_organizations",
            "codex_cli_simplified_flow",
            "originator"
        ]
    );
    assert_eq!(FALLBACK_CALLBACK_PORT, 1457);
}

#[tokio::test]
async fn browser_callback_accepts_only_matching_state() {
    let (result, response) = send_callback("state-test", "state-test").await;
    assert_eq!(result.expect("callback code"), "code-test");
    assert!(response.contains("200 OK"));

    let (result, response) = send_callback("state-test", "wrong-state").await;
    assert!(result.is_err());
    assert!(response.contains("400 Bad Request"));
}

async fn send_callback(
    expected_state: &'static str,
    supplied_state: &str,
) -> (Result<String>, String) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("callback listener");
    let address = listener.local_addr().expect("callback address");
    let redirect_uri = browser_redirect_uri(&listener).unwrap();
    let task = tokio::spawn(async move {
        receive_browser_callback(listener, expected_state, &redirect_uri).await
    });
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect callback");
    let request = format!(
        "GET /auth/callback?code=code-test&state={supplied_state} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write callback");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read callback response");
    (task.await.expect("callback task"), response)
}

#[tokio::test]
async fn browser_flow_reuses_numeric_redirect_for_authorize_and_exchange() {
    let callback = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirect = browser_redirect_uri(&callback).unwrap();
    let expected = redirect.clone();
    let mock = spawn_loopback(Router::new().route("/oauth/token",post(move |body: String| {
        let expected=expected.clone();
        async move {
            let fields: HashMap<_,_> = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect();
            assert_eq!(fields.get("redirect_uri"),Some(&expected));
            Json(serde_json::json!({"id_token":test_jwt(Some("synthetic-browser"),3600),"access_token":test_jwt(None,3600),"refresh_token":"synthetic-browser-refresh"}))
        }
    }))).await;
    let config = OAuthConfig {
        issuer: mock.base_url.clone(),
        client_id: "synthetic-client".into(),
        upstream_url: format!("{}/responses", mock.base_url),
        fingerprint_mode: crate::fingerprint::FingerprintMode::Device,
    };
    let pkce = generate_pkce();
    let url = authorize_url(&config, &redirect, &pkce, "synthetic-state").unwrap();
    let authorized = url
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .unwrap()
        .1
        .into_owned();
    assert_eq!(authorized, redirect);
    let parsed = Url::parse(&authorized).unwrap();
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert_eq!(parsed.port(), Some(callback.local_addr().unwrap().port()));
    let client = Client::builder().no_proxy().build().unwrap();
    exchange_code(&client, &config, &authorized, &pkce, "synthetic-code")
        .await
        .unwrap();
}

#[test]
fn browser_callback_rejects_duplicate_fields_encoded_paths_and_wrong_authorities() {
    let expected = Url::parse("http://127.0.0.1:32145/auth/callback").unwrap();
    let valid = "/auth/callback?code=synthetic-code&state=synthetic-state";
    let parse = |target: &str, host: &str| {
        parse_browser_callback(
            format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes(),
            "synthetic-state",
            &expected,
        )
    };
    assert_eq!(parse(valid, "127.0.0.1:32145").unwrap(), "synthetic-code");
    for target in [
        "/auth/callback?code=one&code=two&state=synthetic-state",
        "/auth/callback?code=one&state=wrong&state=synthetic-state",
        "/auth/callback?code=one&state=synthetic-state&error=denied",
        "/auth/callback?error=one&error=two&state=synthetic-state",
        "/auth/callback?code=one&state=wrong",
        "/auth/callback?code=&state=synthetic-state",
        "/auth/%63allback?code=one&state=synthetic-state",
        "/auth/callback%2f?code=one&state=synthetic-state",
        "//127.0.0.1:32145/auth/callback?code=one&state=synthetic-state",
    ] {
        assert!(parse(target, "127.0.0.1:32145").is_err());
    }
    for host in [
        "127.0.0.1:32146",
        "localhost:32145",
        "127.0.0.1",
        "127.0.0.1:32145\r\nHost: 127.0.0.1:32145",
    ] {
        assert!(parse(valid, host).is_err());
    }
    let legacy = Url::parse("http://localhost:32145/auth/callback").unwrap();
    assert!(
        parse_browser_callback(
            format!("GET {valid} HTTP/1.1\r\nHost: localhost:32145\r\n\r\n").as_bytes(),
            "synthetic-state",
            &legacy
        )
        .is_ok()
    );
}
