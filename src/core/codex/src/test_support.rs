use axum::Router;
use base64::Engine;
use chrono::Utc;
use tokio::task::JoinHandle;
use url::Url;

pub struct LoopbackServer {
    pub base_url: String,
    task: JoinHandle<()>,
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn spawn_loopback(app: Router) -> LoopbackServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback mock");
    let address = listener.local_addr().expect("mock address");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve loopback mock");
    });
    let server = LoopbackServer {
        base_url: format!("http://{address}"),
        task,
    };
    assert_loopback_url(&server.base_url);
    server
}

pub fn assert_loopback_url(raw: &str) {
    let url = Url::parse(raw).expect("parse mock URL");
    let host = url.host_str().expect("mock URL host");
    let ip: std::net::IpAddr = host.parse().expect("mock URL must use a literal IP");
    assert!(ip.is_loopback(), "tests may use only loopback endpoints");
}

pub fn test_jwt(account_id: Option<&str>, expires_in_seconds: i64) -> String {
    let mut claims = serde_json::json!({
        "exp": Utc::now().timestamp() + expires_in_seconds,
    });
    if let Some(account_id) = account_id {
        claims["https://api.openai.com/auth"] =
            serde_json::json!({"chatgpt_account_id": account_id});
    }
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&claims).expect("encode claims"));
    format!("test.{payload}.signature")
}
