use crate::oauth::OAuthConfig;
use crate::oauth::account_id_from_token;
use crate::oauth::token_expiration;
use crate::vault::CredentialMaterial;
use crate::vault::CredentialMetadata;
use crate::vault::Vault;
use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use std::io::Read;
use std::path::Path;

const MAX_AUTH_FILE_BYTES: u64 = 128 * 1024;

#[derive(Deserialize)]
struct CodexAuthFile {
    auth_mode: String,
    tokens: CodexTokens,
}

#[derive(Deserialize)]
struct CodexTokens {
    id_token: String,
    access_token: String,
    account_id: Option<String>,
}

pub async fn import(
    vault: &Vault,
    auth_file: &Path,
    config: OAuthConfig,
) -> Result<CredentialMetadata> {
    let auth_file = auth_file.to_path_buf();
    let auth: CodexAuthFile = tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&auth_file).context("opening Codex auth file")?;
        let metadata = file.metadata().context("reading Codex auth metadata")?;
        anyhow::ensure!(metadata.is_file(), "Codex auth path is not a regular file");
        anyhow::ensure!(
            metadata.len() <= MAX_AUTH_FILE_BYTES,
            "Codex auth file is too large"
        );
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_AUTH_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("reading Codex auth file")?;
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_AUTH_FILE_BYTES,
            "Codex auth file is too large"
        );
        serde_json::from_slice(&bytes).context("decoding Codex auth file")
    })
    .await
    .context("Codex auth import task failed")??;

    anyhow::ensure!(
        auth.auth_mode == "chatgpt",
        "Codex auth file is not a ChatGPT subscription login"
    );
    ensure_present("ID token", &auth.tokens.id_token)?;
    ensure_present("access token", &auth.tokens.access_token)?;

    let account_id = account_id_from_token(&auth.tokens.id_token)
        .context("Codex ID token does not contain a ChatGPT account id")?;
    if let Some(stored_account_id) = auth.tokens.account_id.as_deref() {
        anyhow::ensure!(
            stored_account_id == account_id,
            "Codex auth account identity mismatch"
        );
    }
    let access_expires_at = token_expiration(&auth.tokens.access_token)
        .context("decoding Codex access-token expiration")?
        .context("Codex access token does not contain an expiration")?;

    let material = CredentialMaterial::CodexOAuth {
        id_token: auth.tokens.id_token,
        access_token: auth.tokens.access_token,
        refresh_token: String::new(),
        account_id,
        access_expires_at: Some(access_expires_at),
        issuer: config.issuer,
        client_id: config.client_id,
    };
    vault.create_oauth(material, config.upstream_url).await
}

fn ensure_present(label: &str, value: &str) -> Result<()> {
    anyhow::ensure!(!value.trim().is_empty(), "Codex auth {label} is empty");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_jwt;
    use crate::vault::CredentialMaterial;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn imports_chatgpt_tokens_without_exposing_them_in_metadata() {
        let temp = tempfile::tempdir().expect("temp dir");
        let auth_file = temp.path().join("auth.json");
        let id_token = test_jwt(Some("chatgpt-import-account"), 7200);
        let access_token = test_jwt(None, 3600);
        std::fs::write(
            &auth_file,
            serde_json::to_vec(&serde_json::json!({
                "auth_mode": "chatgpt",
                "tokens": {
                    "id_token": id_token,
                    "access_token": access_token,
                    "refresh_token": "refresh-import-secret",
                    "account_id": "chatgpt-import-account"
                }
            }))
            .expect("auth json"),
        )
        .expect("write auth file");
        let vault = Vault::open(temp.path().join("vault")).expect("vault");

        let metadata = import(
            &vault,
            &auth_file,
            OAuthConfig {
                issuer: "https://auth.openai.com".to_string(),
                client_id: "client-import-test".to_string(),
                upstream_url: "https://chatgpt.com/backend-api/codex/responses".to_string(),
            },
        )
        .await
        .expect("import auth");

        assert_eq!(metadata.auth_kind, "codex_oauth");
        assert_eq!(
            metadata.upstream_account_id.as_deref(),
            Some("chatgpt-import-account")
        );
        let locked = vault
            .lock_record(&metadata.account_ref)
            .await
            .expect("record");
        match &locked.record.material {
            CredentialMaterial::CodexOAuth { refresh_token, .. } => {
                assert!(refresh_token.is_empty())
            }
            CredentialMaterial::OpenAiApiKey { .. } => panic!("wrong credential kind"),
        }
        let serialized = serde_json::to_string(&metadata).expect("metadata json");
        assert!(!serialized.contains("refresh-import-secret"));
        let record = serde_json::to_string(&locked.record).expect("record json");
        assert!(!record.contains("refresh-import-secret"));
    }

    #[tokio::test]
    async fn rejects_mismatched_account_identity() {
        let temp = tempfile::tempdir().expect("temp dir");
        let auth_file = temp.path().join("auth.json");
        std::fs::write(
            &auth_file,
            serde_json::to_vec(&serde_json::json!({
                "auth_mode": "chatgpt",
                "tokens": {
                    "id_token": test_jwt(Some("token-account"), 7200),
                    "access_token": test_jwt(None, 3600),
                    "refresh_token": "refresh-secret",
                    "account_id": "different-account"
                }
            }))
            .expect("auth json"),
        )
        .expect("write auth file");
        let vault = Vault::open(temp.path().join("vault")).expect("vault");

        let error = import(
            &vault,
            &auth_file,
            OAuthConfig {
                issuer: "https://auth.openai.com".to_string(),
                client_id: "client-import-test".to_string(),
                upstream_url: "https://chatgpt.com/backend-api/codex/responses".to_string(),
            },
        )
        .await
        .expect_err("identity mismatch");

        assert!(error.to_string().contains("identity mismatch"));
    }
}
