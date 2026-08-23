use super::*;
use crate::test_support::spawn_loopback;
use crate::test_support::test_jwt;
use axum::Json;
use axum::Router;
use axum::routing::post;

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
                move || {
                    let id_token = id_token.clone();
                    let access_token = access_token.clone();
                    async move {
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
    let task =
        tokio::spawn(async move { receive_browser_callback(listener, expected_state).await });
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect callback");
    let request = format!(
        "GET /auth/callback?code=code-test&state={supplied_state} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
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
