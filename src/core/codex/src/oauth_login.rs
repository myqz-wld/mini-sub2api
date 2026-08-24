use crate::http_body::decode_auth_json;
use crate::http_client::apply_loopback_proxy_policy;
use crate::oauth::LoginFlow;
use crate::oauth::OAuthConfig;
use crate::oauth::Pkce;
use crate::oauth::account_id_from_token;
use crate::oauth::generate_pkce;
use crate::oauth::random_urlsafe;
use crate::oauth::token_expiration;
use crate::vault::CredentialMaterial;
use crate::vault::CredentialMetadata;
use crate::vault::Vault;
use anyhow::Context;
use anyhow::Result;
use reqwest::Client;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use url::Url;

const DEFAULT_CALLBACK_PORT: u16 = 1455;
const FALLBACK_CALLBACK_PORT: u16 = 1457;

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
    access_token: String,
    refresh_token: String,
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    #[serde(default, deserialize_with = "deserialize_u64_string_or_number")]
    interval: u64,
}

#[derive(Deserialize)]
struct DeviceAuthorizationResponse {
    authorization_code: String,
    code_challenge: String,
    code_verifier: String,
}

pub(crate) async fn login(
    vault: &Vault,
    flow: LoginFlow,
    config: OAuthConfig,
) -> Result<CredentialMetadata> {
    validate_auth_url(&config.issuer)?;
    validate_auth_url(&config.upstream_url)?;
    let client = apply_loopback_proxy_policy(
        Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none()),
        &config.issuer,
    )
    .build()
    .context("building OAuth client")?;
    let tokens = match flow {
        LoginFlow::Device => device_login(&client, &config).await?,
        LoginFlow::Browser => browser_login(&client, &config).await?,
    };
    let account_id = account_id_from_token(&tokens.id_token)
        .context("OAuth ID token does not contain a ChatGPT account id")?;
    let access_expires_at = token_expiration(&tokens.access_token)
        .context("decoding OAuth access-token expiration")?
        .context("OAuth access token does not contain an expiration")?;
    let material = CredentialMaterial::CodexOAuth {
        id_token: tokens.id_token,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        account_id,
        access_expires_at: Some(access_expires_at),
        issuer: config.issuer,
        client_id: config.client_id,
    };
    vault
        .create_oauth(material, config.upstream_url, config.fingerprint_mode)
        .await
}

async fn device_login(client: &Client, config: &OAuthConfig) -> Result<TokenResponse> {
    let issuer = config.issuer.trim_end_matches('/');
    let response = client
        .post(format!("{issuer}/api/accounts/deviceauth/usercode"))
        .json(&serde_json::json!({"client_id": &config.client_id}))
        .send()
        .await
        .context("requesting device code")?;
    if !response.status().is_success() {
        anyhow::bail!("device-code login is unavailable ({})", response.status());
    }
    let device: DeviceCodeResponse = decode_auth_json(response, "device code response").await?;
    eprintln!(
        "Open {issuer}/codex/device and enter code {}. The code expires in 15 minutes.",
        device.user_code
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15 * 60);
    let authorization = loop {
        let response = client
            .post(format!("{issuer}/api/accounts/deviceauth/token"))
            .json(&serde_json::json!({
                "device_auth_id": &device.device_auth_id,
                "user_code": &device.user_code
            }))
            .send()
            .await
            .context("polling device authorization")?;
        if response.status().is_success() {
            break decode_auth_json::<DeviceAuthorizationResponse>(
                response,
                "device authorization response",
            )
            .await?;
        }
        if !matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ) {
            anyhow::bail!("device authorization returned {}", response.status());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("device authorization timed out");
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        tokio::time::sleep(Duration::from_secs(device.interval.max(1)).min(remaining)).await;
    };
    let pkce = Pkce {
        verifier: authorization.code_verifier,
        challenge: authorization.code_challenge,
    };
    exchange_code(
        client,
        config,
        &format!("{issuer}/deviceauth/callback"),
        &pkce,
        &authorization.authorization_code,
    )
    .await
}

async fn browser_login(client: &Client, config: &OAuthConfig) -> Result<TokenResponse> {
    let listener = bind_browser_listener().await?;
    let redirect_uri = format!(
        "http://localhost:{}/auth/callback",
        listener.local_addr()?.port()
    );
    let pkce = generate_pkce();
    let state = random_urlsafe(32);
    let auth_url = authorize_url(config, &redirect_uri, &pkce, &state)?;
    eprintln!("Open this URL to sign in:\n{auth_url}");
    let _ = webbrowser::open(auth_url.as_str());

    let callback = tokio::time::timeout(Duration::from_secs(10 * 60), async {
        receive_browser_callback(listener, &state).await
    })
    .await
    .context("OAuth callback timed out")??;
    exchange_code(client, config, &redirect_uri, &pkce, &callback).await
}

async fn receive_browser_callback(listener: TcpListener, state: &str) -> Result<String> {
    let (mut socket, _) = listener.accept().await?;
    let mut request = Vec::with_capacity(4096);
    let mut complete = false;
    loop {
        let mut chunk = [0_u8; 1024];
        let count = socket.read(&mut chunk).await?;
        if count == 0 || request.len() + count > 8192 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            complete = true;
            break;
        }
    }
    anyhow::ensure!(
        complete && request.len() <= 8192,
        "invalid OAuth callback request"
    );
    let first_line = String::from_utf8_lossy(&request)
        .lines()
        .next()
        .context("missing OAuth callback request line")?
        .to_string();
    let mut request_parts = first_line.split_whitespace();
    anyhow::ensure!(
        request_parts.next() == Some("GET"),
        "invalid OAuth callback method"
    );
    let target = request_parts
        .next()
        .context("missing OAuth callback target")?;
    let url = Url::parse(&format!("http://localhost{target}"))?;
    anyhow::ensure!(
        url.path() == "/auth/callback",
        "invalid OAuth callback path"
    );
    let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
    let valid_state = params.get("state").is_some_and(|value| value == state);
    let code = params
        .get("code")
        .filter(|value| !value.is_empty())
        .cloned();
    let (status, body) = if valid_state && code.is_some() {
        ("200 OK", "Sign-in completed. You may close this window.")
    } else {
        ("400 Bad Request", "Sign-in failed.")
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    anyhow::ensure!(valid_state, "OAuth state mismatch");
    code.context("OAuth callback did not contain a code")
}

async fn exchange_code(
    client: &Client,
    config: &OAuthConfig,
    redirect_uri: &str,
    pkce: &Pkce,
    code: &str,
) -> Result<TokenResponse> {
    let response = client
        .post(format!(
            "{}/oauth/token",
            config.issuer.trim_end_matches('/')
        ))
        .header(
            http::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
            urlencoding::encode(code),
            urlencoding::encode(redirect_uri),
            urlencoding::encode(&config.client_id),
            urlencoding::encode(&pkce.verifier)
        ))
        .send()
        .await
        .context("exchanging OAuth code")?;
    if !response.status().is_success() {
        anyhow::bail!("OAuth token exchange returned {}", response.status());
    }
    decode_auth_json(response, "OAuth token response").await
}

fn authorize_url(
    config: &OAuthConfig,
    redirect_uri: &str,
    pkce: &Pkce,
    state: &str,
) -> Result<Url> {
    let query = [
        ("response_type", "code"),
        ("client_id", config.client_id.as_str()),
        ("redirect_uri", redirect_uri),
        (
            "scope",
            "openid profile email offline_access api.connectors.read api.connectors.invoke",
        ),
        ("code_challenge", pkce.challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", state),
        (
            "originator",
            crate::upstream_request::DEFAULT_CODEX_ORIGINATOR,
        ),
    ]
    .into_iter()
    .map(|(name, value)| format!("{name}={}", urlencoding::encode(value)))
    .collect::<Vec<_>>()
    .join("&");
    Url::parse(&format!(
        "{}/oauth/authorize?{query}",
        config.issuer.trim_end_matches('/')
    ))
    .context("building OAuth authorize URL")
}

async fn bind_browser_listener() -> Result<TcpListener> {
    let address = IpAddr::from([127, 0, 0, 1]);
    match TcpListener::bind((address, DEFAULT_CALLBACK_PORT)).await {
        Ok(listener) => Ok(listener),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            TcpListener::bind((address, FALLBACK_CALLBACK_PORT))
                .await
                .context("binding fallback OAuth callback")
        }
        Err(error) => Err(error).context("binding OAuth callback"),
    }
}

fn validate_auth_url(raw: &str) -> Result<()> {
    let url = Url::parse(raw).context("parsing configured URL")?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https"),
        "unsupported URL scheme"
    );
    anyhow::ensure!(url.host_str().is_some(), "configured URL has no host");
    anyhow::ensure!(
        url.scheme() == "https" || url_host_is_loopback(&url),
        "plain HTTP is allowed only for loopback compatibility endpoints"
    );
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "configured URL must not contain user info"
    );
    Ok(())
}

fn url_host_is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(_)) | None => false,
    }
}

fn deserialize_u64_string_or_number<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| serde::de::Error::custom("invalid interval")),
        Value::String(text) => text.parse().map_err(serde::de::Error::custom),
        _ => Err(serde::de::Error::custom("invalid interval")),
    }
}

#[cfg(test)]
#[path = "oauth_login_tests.rs"]
mod tests;
