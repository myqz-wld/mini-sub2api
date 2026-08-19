use super::*;
use crate::test_support::spawn_loopback;
use crate::test_support::test_jwt;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode as AxumStatusCode;
use axum::routing::post;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn concurrent_expiry_refresh_rotates_once_and_persists() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let account_id = "chatgpt-refresh-test";
    let new_id = test_jwt(Some(account_id), 3600);
    let new_access = test_jwt(None, 3600);
    let app = Router::new().route(
        "/oauth/token",
        post({
            let refreshes = Arc::clone(&refreshes);
            let new_id = new_id.clone();
            let new_access = new_access.clone();
            move || {
                let refreshes = Arc::clone(&refreshes);
                let new_id = new_id.clone();
                let new_access = new_access.clone();
                async move {
                    refreshes.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({
                        "id_token": new_id,
                        "access_token": new_access,
                        "refresh_token": "refresh-rotated-test"
                    }))
                }
            }
        }),
    );
    let mock = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_oauth(
            oauth_material(account_id, &mock.base_url, -3600),
            format!("{}/responses", mock.base_url),
        )
        .await
        .expect("create OAuth record");
    let client = Client::builder().build().expect("client");

    let mut tasks = Vec::new();
    for _ in 0..12 {
        let vault = vault.clone();
        let account_ref = metadata.account_ref.clone();
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            let mut locked = vault.lock_record(&account_ref).await.expect("lock record");
            refresh_if_needed(&mut locked, &client, false)
                .await
                .expect("refresh");
        }));
    }
    for task in tasks {
        task.await.expect("refresh task");
    }

    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    let locked = vault
        .lock_record(&metadata.account_ref)
        .await
        .expect("final record");
    match &locked.record.material {
        CredentialMaterial::CodexOAuth {
            access_token,
            refresh_token,
            ..
        } => {
            assert_eq!(access_token, &new_access);
            assert_eq!(refresh_token, "refresh-rotated-test");
        }
        CredentialMaterial::OpenAiApiKey { .. } => panic!("wrong credential kind"),
    }
}

#[tokio::test]
async fn permanent_refresh_failure_marks_requires_login() {
    let app = Router::new().route(
        "/oauth/token",
        post(|| async {
            (
                AxumStatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {"code": "refresh_token_reused"}
                })),
            )
        }),
    );
    let mock = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_oauth(
            oauth_material("chatgpt-permanent-test", &mock.base_url, -3600),
            format!("{}/responses", mock.base_url),
        )
        .await
        .expect("create OAuth record");
    let client = Client::builder().build().expect("client");
    let mut locked = vault
        .lock_record(&metadata.account_ref)
        .await
        .expect("lock record");

    let error = refresh_if_needed(&mut locked, &client, false)
        .await
        .expect_err("permanent refresh failure");
    assert!(matches!(error, OAuthFailure::RequiresLogin));
    drop(locked);
    let locked = vault
        .lock_record(&metadata.account_ref)
        .await
        .expect("persisted status");
    assert_eq!(locked.record.status, CredentialStatus::RequiresLogin);
}

#[tokio::test]
async fn unparseable_rotated_access_token_marks_requires_login() {
    let account_id = "chatgpt-invalid-rotation-test";
    let id_token = test_jwt(Some(account_id), 3600);
    let app = Router::new().route(
        "/oauth/token",
        post(move || {
            let id_token = id_token.clone();
            async move {
                Json(serde_json::json!({
                    "id_token": id_token,
                    "access_token": "not-a-jwt",
                    "refresh_token": "refresh-rotated-test"
                }))
            }
        }),
    );
    let mock = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_oauth(
            oauth_material(account_id, &mock.base_url, -3600),
            format!("{}/responses", mock.base_url),
        )
        .await
        .expect("create OAuth record");
    let mut locked = vault
        .lock_record(&metadata.account_ref)
        .await
        .expect("lock record");
    let error = refresh_if_needed(&mut locked, &Client::new(), false)
        .await
        .expect_err("invalid rotated token");
    assert!(matches!(error, OAuthFailure::RequiresLogin));
    assert_eq!(locked.record.status, CredentialStatus::RequiresLogin);
}

#[derive(Clone)]
struct RevokeCapture(Arc<tokio::sync::Mutex<Option<Value>>>);

#[tokio::test]
async fn revoke_posts_refresh_token_to_loopback_authority() {
    let capture = RevokeCapture(Arc::new(tokio::sync::Mutex::new(None)));
    let app = Router::new()
        .route(
            "/oauth/revoke",
            post(
                |State(capture): State<RevokeCapture>, Json(body): Json<Value>| async move {
                    *capture.0.lock().await = Some(body);
                    AxumStatusCode::OK
                },
            ),
        )
        .with_state(capture.clone());
    let mock = spawn_loopback(app).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(temp.path().to_path_buf()).expect("vault");
    let metadata = vault
        .create_oauth(
            oauth_material("chatgpt-revoke-test", &mock.base_url, 3600),
            format!("{}/responses", mock.base_url),
        )
        .await
        .expect("create OAuth record");
    let mut locked = vault
        .lock_record(&metadata.account_ref)
        .await
        .expect("lock record");

    revoke(&mut locked, &Client::new()).await.expect("revoke");
    let body = capture.0.lock().await.clone().expect("captured body");
    assert_eq!(body["token"], "refresh-old-test");
    assert_eq!(body["token_type_hint"], "refresh_token");
    assert_eq!(body["client_id"], "client-test");
}

fn oauth_material(account_id: &str, issuer: &str, expires_in: i64) -> CredentialMaterial {
    CredentialMaterial::CodexOAuth {
        id_token: test_jwt(Some(account_id), 3600),
        access_token: test_jwt(None, expires_in),
        refresh_token: "refresh-old-test".to_string(),
        account_id: account_id.to_string(),
        access_expires_at: Some(Utc::now() + Duration::seconds(expires_in)),
        issuer: issuer.to_string(),
        client_id: "client-test".to_string(),
    }
}
