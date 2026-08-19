use crate::http_body::decode_auth_json;
use crate::http_body::read_auth_error;
use crate::vault::CredentialMaterial;
use crate::vault::CredentialMetadata;
use crate::vault::CredentialStatus;
use crate::vault::LockedRecord;
use crate::vault::Vault;
use anyhow::Context;
use anyhow::Result;
use base64::Engine;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use rand::RngCore;
use reqwest::Client;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use std::path::PathBuf;
use std::time::Duration as StdDuration;

pub const DEFAULT_ISSUER: &str = "https://auth.openai.com";
pub const DEFAULT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[derive(Clone)]
pub struct OAuthConfig {
    pub issuer: String,
    pub client_id: String,
    pub upstream_url: String,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum LoginFlow {
    Device,
    Browser,
}

#[derive(Debug, thiserror::Error)]
pub enum OAuthFailure {
    #[error("credential requires sign-in")]
    RequiresLogin,
    #[error("OAuth transport failed")]
    Transport(#[source] anyhow::Error),
}

pub(crate) struct Pkce {
    pub(crate) verifier: String,
    pub(crate) challenge: String,
}

#[derive(Deserialize)]
struct RefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Serialize)]
struct RefreshRequest<'a> {
    client_id: &'a str,
    grant_type: &'static str,
    refresh_token: &'a str,
}

pub async fn login(
    vault: &Vault,
    flow: LoginFlow,
    config: OAuthConfig,
) -> Result<CredentialMetadata> {
    crate::oauth_login::login(vault, flow, config).await
}

pub async fn refresh_if_needed(
    locked: &mut LockedRecord,
    client: &Client,
    force: bool,
) -> Result<(), OAuthFailure> {
    if locked.record.status == CredentialStatus::RequiresLogin {
        return Err(OAuthFailure::RequiresLogin);
    }
    let (access_expires_at, issuer, client_id, refresh_token, account_id) =
        match &locked.record.material {
            CredentialMaterial::CodexOAuth {
                access_expires_at,
                issuer,
                client_id,
                refresh_token,
                account_id,
                ..
            } => (
                *access_expires_at,
                issuer.clone(),
                client_id.clone(),
                refresh_token.clone(),
                account_id.clone(),
            ),
            CredentialMaterial::OpenAiApiKey { .. } => return Ok(()),
        };

    if !force
        && access_expires_at.is_some_and(|expires| expires > Utc::now() + Duration::minutes(5))
    {
        return Ok(());
    }

    let endpoint = format!("{}/oauth/token", issuer.trim_end_matches('/'));
    let response = client
        .post(&endpoint)
        .json(&RefreshRequest {
            client_id: &client_id,
            grant_type: "refresh_token",
            refresh_token: &refresh_token,
        })
        .send()
        .await
        .map_err(|error| OAuthFailure::Transport(anyhow::Error::new(error)))?;
    let status = response.status();
    if !status.is_success() {
        if status == StatusCode::UNAUTHORIZED {
            return mark_requires_login(locked).await;
        }
        let body = read_auth_error(response)
            .await
            .map_err(OAuthFailure::Transport)?;
        if is_permanent_refresh_error(&body) {
            return mark_requires_login(locked).await;
        }
        return Err(OAuthFailure::Transport(anyhow::anyhow!(
            "token refresh returned {status}"
        )));
    }

    let refreshed: RefreshResponse = decode_auth_json(response, "OAuth refresh response")
        .await
        .map_err(OAuthFailure::Transport)?;
    if let Some(new_id_token) = refreshed.id_token.as_deref() {
        let Some(new_account_id) = account_id_from_token(new_id_token) else {
            return mark_requires_login(locked).await;
        };
        if new_account_id != account_id {
            return mark_requires_login(locked).await;
        }
    }
    let new_access_expiration = if let Some(new_access_token) = refreshed.access_token.as_deref() {
        match token_expiration(new_access_token) {
            Ok(Some(expiration)) => Some(expiration),
            Ok(None) | Err(_) => return mark_requires_login(locked).await,
        }
    } else {
        None
    };

    let CredentialMaterial::CodexOAuth {
        id_token,
        access_token,
        refresh_token,
        access_expires_at,
        ..
    } = &mut locked.record.material
    else {
        return Err(OAuthFailure::Transport(anyhow::anyhow!(
            "credential kind changed while refreshing"
        )));
    };
    if let Some(new_id_token) = refreshed.id_token {
        *id_token = new_id_token;
    }
    if let Some(new_access_token) = refreshed.access_token {
        *access_expires_at = new_access_expiration;
        *access_token = new_access_token;
    }
    if let Some(new_refresh_token) = refreshed.refresh_token {
        *refresh_token = new_refresh_token;
    }
    locked.persist().await.map_err(OAuthFailure::Transport)
}

async fn mark_requires_login(locked: &mut LockedRecord) -> Result<(), OAuthFailure> {
    locked.record.status = CredentialStatus::RequiresLogin;
    locked.persist().await.map_err(OAuthFailure::Transport)?;
    Err(OAuthFailure::RequiresLogin)
}

pub async fn revoke(locked: &mut LockedRecord, client: &Client) -> Result<()> {
    let CredentialMaterial::CodexOAuth {
        refresh_token,
        access_token,
        issuer,
        client_id,
        ..
    } = &locked.record.material
    else {
        anyhow::bail!("credential is not OAuth-backed");
    };
    let (token, hint, request_client_id) = if refresh_token.is_empty() {
        (access_token.as_str(), "access_token", None)
    } else {
        (
            refresh_token.as_str(),
            "refresh_token",
            Some(client_id.as_str()),
        )
    };
    let endpoint = format!("{}/oauth/revoke", issuer.trim_end_matches('/'));
    let mut body = serde_json::json!({"token": token, "token_type_hint": hint});
    if let Some(client_id) = request_client_id {
        body["client_id"] = Value::String(client_id.to_string());
    }
    let response = client
        .post(endpoint)
        .timeout(StdDuration::from_secs(10))
        .json(&body)
        .send()
        .await
        .context("sending OAuth revoke")?;
    if !response.status().is_success() {
        anyhow::bail!("OAuth revoke returned {}", response.status());
    }
    Ok(())
}

pub fn access_token_and_account(record: &crate::vault::VaultRecord) -> Option<(&str, &str)> {
    match &record.material {
        CredentialMaterial::CodexOAuth {
            access_token,
            account_id,
            ..
        } => Some((access_token, account_id)),
        CredentialMaterial::OpenAiApiKey { .. } => None,
    }
}

pub(crate) fn generate_pkce() -> Pkce {
    let verifier = random_urlsafe(64);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    Pkce {
        verifier,
        challenge,
    }
}

pub(crate) fn random_urlsafe(size: usize) -> String {
    let mut bytes = vec![0_u8; size];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn account_id_from_token(token: &str) -> Option<String> {
    let claims = jwt_claims(token).ok()?;
    claims
        .get("https://api.openai.com/auth")
        .and_then(|value| value.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(crate) fn token_expiration(token: &str) -> Result<Option<DateTime<Utc>>> {
    let claims = jwt_claims(token)?;
    Ok(claims
        .get("exp")
        .and_then(Value::as_i64)
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0)))
}

fn jwt_claims(token: &str) -> Result<Value> {
    let payload = token.split('.').nth(1).context("invalid JWT format")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .context("decoding JWT payload")?;
    serde_json::from_slice(&bytes).context("decoding JWT claims")
}

fn is_permanent_refresh_error(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let code = value
        .get("error")
        .and_then(|error| error.get("code"))
        .or_else(|| value.get("code"))
        .and_then(Value::as_str);
    matches!(
        code,
        Some("refresh_token_expired" | "refresh_token_reused" | "refresh_token_invalidated")
    )
}

pub fn default_state_dir() -> PathBuf {
    std::env::var_os("MINI_SUB2API_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".mini-sub2api"))
        .join("core-codex")
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "oauth_integration_tests.rs"]
mod integration_tests;
