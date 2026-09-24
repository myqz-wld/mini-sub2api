use super::*;
use axum::extract::ConnectInfo;
use http_body_util::BodyExt;
use std::net::SocketAddr;

#[tokio::test]
async fn auth_recovery_rebuilds_each_model_client_even_when_reload_keeps_token() {
    for unauthorized in [1usize, 2] {
        let peers = Arc::new(Mutex::new(Vec::new()));
        let refreshed = Arc::new(AtomicUsize::new(0));
        let access = test_jwt(None, 7200);
        let old = test_jwt(None, 3600);
        let app = Router::new().route("/responses", axum_post({
            let peers = peers.clone(); let new = access.clone(); let old = old.clone();
            move |ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap| {
                let peers = peers.clone(); let new = new.clone(); let old = old.clone();
                async move {
                    let mut peers = peers.lock().await;
                    let attempt = peers.len(); peers.push(peer);
                    let token = if unauthorized == 2 && attempt == 2 { new } else { old };
                    assert_eq!(headers[http::header::AUTHORIZATION], format!("Bearer {token}"));
                    if attempt < unauthorized { return (StatusCode::UNAUTHORIZED, ""); }
                    (StatusCode::OK, "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_recovery\",\"output\":[]}}\n\n")
                }
            }
        })).route("/oauth/token", axum_post({
            let refreshed = refreshed.clone(); let access = access.clone();
            move || { let refreshed = refreshed.clone(); let access = access.clone(); async move {
                refreshed.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({"access_token":access, "id_token":test_jwt(Some("synthetic-auth-boundary"),7200),"refresh_token":"synthetic-new-refresh"}))
            }}
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        crate::test_support::assert_loopback_url(&url);
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let vault = Vault::open(temp.path().into()).unwrap();
        let account = vault
            .create_oauth(
                CredentialMaterial::CodexOAuth {
                    id_token: test_jwt(Some("synthetic-auth-boundary"), 7200),
                    access_token: old,
                    refresh_token: "synthetic-refresh".into(),
                    account_id: "synthetic-auth-boundary".into(),
                    access_expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                    issuer: url.clone(),
                    client_id: "synthetic-client".into(),
                },
                format!("{url}/responses"),
                crate::fingerprint::FingerprintMode::Device,
            )
            .await
            .unwrap();
        let response = call_core(
            &app_state(vault),
            &account.account_ref,
            Bytes::from_static(br#"{"model":"gpt-5.4","input":"synthetic"}"#),
        )
        .await
        .unwrap();
        response.into_body().collect().await.unwrap();
        let peers = peers.lock().await;
        assert_eq!(peers.len(), unauthorized + 1);
        assert_eq!(
            peers
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            peers.len()
        );
        assert_eq!(refreshed.load(Ordering::SeqCst), unauthorized - 1);
        server.abort();
    }
}
