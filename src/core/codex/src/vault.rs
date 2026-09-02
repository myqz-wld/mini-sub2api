use crate::fingerprint::{self, FingerprintMetadata, FingerprintMode, FingerprintSnapshot};
use crate::request_state_store::RequestStateStore;
use crate::request_state_store::cleanup_orphan_request_states;
use crate::vault_io::open_private_file;
use crate::vault_io::read_json_limited;
use crate::vault_io::set_private_directory;
use crate::vault_io::sync_directory;
use crate::vault_io::write_json_atomically;
use anyhow::Context;
use anyhow::Result;
use chrono::DateTime;
use chrono::Utc;
use fs2::FileExt;
use serde::Deserialize;
use serde::Serialize;
use std::fs::File;
use std::path::Path;
use std::path::PathBuf;
use uuid::Uuid;

#[path = "vault_lifecycle.rs"]
mod lifecycle;
use lifecycle::complete_removal_locked;
use lifecycle::is_not_found;
use lifecycle::read_optional_receipt;
use lifecycle::receipt_path;

pub const DEFAULT_CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const DEFAULT_OPENAI_RESPONSES_URL: &str = "https://api.openai.com/v1/responses";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatus {
    Ready,
    RequiresLogin,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalKind {
    ServiceOnly,
    OAuthRevoked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovalReceipt {
    pub account_ref: String,
    pub kind: RemovalKind,
    pub completed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) request_state_ref: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialMaterial {
    CodexOAuth {
        id_token: String,
        access_token: String,
        refresh_token: String,
        account_id: String,
        access_expires_at: Option<DateTime<Utc>>,
        issuer: String,
        client_id: String,
    },
    OpenAiApiKey {
        api_key: String,
    },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultRecord {
    pub account_ref: String,
    pub status: CredentialStatus,
    pub upstream_url: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub material: CredentialMaterial,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialMetadata {
    pub account_ref: String,
    pub auth_kind: String,
    pub upstream_account_id: Option<String>,
    pub status: CredentialStatus,
}

impl VaultRecord {
    pub fn metadata(&self) -> CredentialMetadata {
        let (auth_kind, upstream_account_id) = match &self.material {
            CredentialMaterial::CodexOAuth { account_id, .. } => {
                ("codex_oauth".to_string(), Some(account_id.clone()))
            }
            CredentialMaterial::OpenAiApiKey { .. } => ("openai_api_key".to_string(), None),
        };
        CredentialMetadata {
            account_ref: self.account_ref.clone(),
            auth_kind,
            upstream_account_id,
            status: self.status,
        }
    }

    pub(crate) fn request_state_namespace(&self) -> &str {
        match &self.material {
            CredentialMaterial::CodexOAuth { account_id, .. } => account_id,
            CredentialMaterial::OpenAiApiKey { .. } => &self.account_ref,
        }
    }
}

#[derive(Clone)]
pub struct Vault {
    state_dir: PathBuf,
    accounts_dir: PathBuf,
    request_state: RequestStateStore,
}

pub struct InstanceLock {
    file: File,
}

pub struct LockedRecord {
    lock_file: File,
    record_path: PathBuf,
    accounts_dir: PathBuf,
    fingerprint: FingerprintSnapshot,
    pub record: VaultRecord,
}

impl Vault {
    pub fn open(state_dir: PathBuf) -> Result<Self> {
        let accounts_dir = state_dir.join("accounts");
        std::fs::create_dir_all(&accounts_dir).context("creating credential vault")?;
        set_private_directory(&state_dir)?;
        set_private_directory(&accounts_dir)?;
        cleanup_orphan_fingerprints(&accounts_dir)?;
        cleanup_orphan_request_states(&accounts_dir)?;
        Ok(Self {
            state_dir,
            request_state: RequestStateStore::new(accounts_dir.clone()),
            accounts_dir,
        })
    }

    pub fn acquire_instance_lock(&self) -> Result<InstanceLock> {
        let path = self.state_dir.join("core-instance.lock");
        let file = open_private_file(&path)?;
        file.try_lock_exclusive()
            .context("another Codex core is using this state directory")?;
        Ok(InstanceLock { file })
    }

    pub async fn create_oauth(
        &self,
        material: CredentialMaterial,
        upstream_url: String,
        fingerprint_mode: FingerprintMode,
    ) -> Result<CredentialMetadata> {
        let record = new_record(material, upstream_url);
        self.create_record(record, fingerprint_mode).await
    }

    pub async fn create_api_key(
        &self,
        api_key: String,
        upstream_url: String,
        fingerprint_mode: FingerprintMode,
    ) -> Result<CredentialMetadata> {
        let record = new_record(CredentialMaterial::OpenAiApiKey { api_key }, upstream_url);
        self.create_record(record, fingerprint_mode).await
    }

    async fn create_record(
        &self,
        record: VaultRecord,
        fingerprint_mode: FingerprintMode,
    ) -> Result<CredentialMetadata> {
        let metadata = record.metadata();
        let accounts_dir = self.accounts_dir.clone();
        tokio::task::spawn_blocking(move || {
            let state_namespace = record.request_state_namespace().to_string();
            let path = record_path(&accounts_dir, &record.account_ref)?;
            let _lock = lock_account(&accounts_dir, &record.account_ref)?;
            if path.exists() {
                anyhow::bail!("credential reference collision");
            }
            anyhow::ensure!(
                !fingerprint::sidecar_path(&accounts_dir, &record.account_ref).exists(),
                "credential fingerprint reference collision"
            );
            anyhow::ensure!(
                !receipt_path(&accounts_dir, &record.account_ref)?.exists(),
                "credential removal reference collision"
            );
            fingerprint::create(&accounts_dir, &record.account_ref, fingerprint_mode)?;
            if let Err(write_error) = write_record_atomically(&accounts_dir, &path, &record) {
                fingerprint::remove_if_exists(&accounts_dir, &record.account_ref)
                    .context("cleaning failed credential creation")?;
                return Err(write_error);
            }
            if let Err(state_error) = RequestStateStore::new(accounts_dir.clone())
                .register_owner_if_exists(&state_namespace, &record.account_ref)
            {
                let _ = std::fs::remove_file(&path);
                fingerprint::remove_if_exists(&accounts_dir, &record.account_ref)
                    .context("cleaning failed request state owner registration")?;
                sync_directory(&accounts_dir);
                return Err(state_error);
            }
            Ok(())
        })
        .await
        .context("credential write task failed")??;
        Ok(metadata)
    }

    pub async fn lock_record(&self, account_ref: &str) -> Result<LockedRecord> {
        validate_account_ref(account_ref)?;
        let account_ref = account_ref.to_string();
        let accounts_dir = self.accounts_dir.clone();
        tokio::task::spawn_blocking(move || {
            let record_path = record_path(&accounts_dir, &account_ref)?;
            let lock_file = lock_account(&accounts_dir, &account_ref)?;
            let record = match read_record(&record_path) {
                Ok(record) => record,
                Err(error) => {
                    if is_not_found(&error)
                        && let Some(receipt) =
                            read_optional_receipt(&receipt_path(&accounts_dir, &account_ref)?)?
                    {
                        anyhow::ensure!(
                            receipt.account_ref == account_ref,
                            "credential removal receipt identity mismatch"
                        );
                        fingerprint::remove_if_exists(&accounts_dir, &account_ref)?;
                    }
                    return Err(error);
                }
            };
            anyhow::ensure!(
                record.account_ref == account_ref,
                "credential record identity mismatch"
            );
            let fingerprint = fingerprint::load_or_materialize(&accounts_dir, &account_ref)?;
            Ok(LockedRecord {
                lock_file,
                record_path,
                accounts_dir,
                fingerprint,
                record,
            })
        })
        .await
        .context("credential lock task failed")?
    }

    pub async fn removal_receipt(&self, account_ref: &str) -> Result<Option<RemovalReceipt>> {
        validate_account_ref(account_ref)?;
        let account_ref = account_ref.to_string();
        let accounts_dir = self.accounts_dir.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock_account(&accounts_dir, &account_ref)?;
            let receipt = read_optional_receipt(&receipt_path(&accounts_dir, &account_ref)?)?;
            if let Some(receipt) = &receipt {
                anyhow::ensure!(
                    receipt.account_ref == account_ref,
                    "credential removal receipt identity mismatch"
                );
                if !record_path(&accounts_dir, &account_ref)?.exists() {
                    fingerprint::remove_if_exists(&accounts_dir, &account_ref)?;
                }
            }
            Ok(receipt)
        })
        .await
        .context("credential receipt read task failed")?
    }

    pub async fn remove(
        &self,
        account_ref: &str,
        requested_kind: RemovalKind,
    ) -> Result<RemovalReceipt> {
        validate_account_ref(account_ref)?;
        let account_ref = account_ref.to_string();
        let accounts_dir = self.accounts_dir.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock_account(&accounts_dir, &account_ref)?;
            let record_path = record_path(&accounts_dir, &account_ref)?;
            complete_removal_locked(&accounts_dir, &record_path, &account_ref, requested_kind)
        })
        .await
        .context("credential removal task failed")?
    }

    pub async fn fingerprint_snapshot(&self, account_ref: &str) -> Result<FingerprintSnapshot> {
        let locked = self.lock_record(account_ref).await?;
        Ok(locked.fingerprint.clone())
    }

    pub(crate) fn request_state(&self) -> &RequestStateStore {
        &self.request_state
    }

    pub async fn fingerprint_metadata(&self, account_ref: &str) -> Result<FingerprintMetadata> {
        let locked = self.lock_record(account_ref).await?;
        Ok(locked.fingerprint.metadata(account_ref))
    }

    pub async fn set_fingerprint_mode(
        &self,
        account_ref: &str,
        mode: FingerprintMode,
    ) -> Result<FingerprintMetadata> {
        let mut locked = self.lock_record(account_ref).await?;
        locked.set_fingerprint_mode(mode).await?;
        Ok(locked.fingerprint.metadata(account_ref))
    }
}

impl LockedRecord {
    pub fn fingerprint(&self) -> &FingerprintSnapshot {
        &self.fingerprint
    }

    pub async fn persist(&mut self) -> Result<()> {
        self.record.updated_at = Utc::now();
        let accounts_dir = self.accounts_dir.clone();
        let record_path = self.record_path.clone();
        let record = self.record.clone();
        tokio::task::spawn_blocking(move || {
            write_record_atomically(&accounts_dir, &record_path, &record)
        })
        .await
        .context("credential persist task failed")?
    }

    pub async fn complete_removal(self, requested_kind: RemovalKind) -> Result<RemovalReceipt> {
        tokio::task::spawn_blocking(move || {
            let account_ref = self.record.account_ref.clone();
            complete_removal_locked(
                &self.accounts_dir,
                &self.record_path,
                &account_ref,
                requested_kind,
            )
        })
        .await
        .context("locked credential removal task failed")?
    }

    pub async fn set_fingerprint_mode(&mut self, mode: FingerprintMode) -> Result<()> {
        let accounts_dir = self.accounts_dir.clone();
        let account_ref = self.record.account_ref.clone();
        let fingerprint = self.fingerprint.clone();
        self.fingerprint = tokio::task::spawn_blocking(move || {
            fingerprint::update_mode(&accounts_dir, &account_ref, &fingerprint, mode)
        })
        .await
        .context("credential fingerprint update task failed")??;
        Ok(())
    }
}

impl Drop for LockedRecord {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.lock_file);
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn new_record(material: CredentialMaterial, upstream_url: String) -> VaultRecord {
    let now = Utc::now();
    VaultRecord {
        account_ref: format!("acct_{}", Uuid::new_v4().simple()),
        status: CredentialStatus::Ready,
        upstream_url,
        created_at: now,
        updated_at: now,
        material,
    }
}

fn validate_account_ref(account_ref: &str) -> Result<()> {
    let suffix = account_ref
        .strip_prefix("acct_")
        .context("invalid account reference")?;
    if suffix.is_empty()
        || suffix.len() > 128
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        anyhow::bail!("invalid account reference");
    }
    Ok(())
}

fn record_path(accounts_dir: &Path, account_ref: &str) -> Result<PathBuf> {
    validate_account_ref(account_ref)?;
    Ok(accounts_dir.join(format!("{account_ref}.json")))
}

fn cleanup_orphan_fingerprints(accounts_dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(accounts_dir).context("scanning credential vault")? {
        let entry = entry.context("reading credential vault entry")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(account_ref) = fingerprint::account_ref_from_sidecar_name(name) else {
            continue;
        };
        if validate_account_ref(account_ref).is_err() {
            continue;
        }
        let _lock = lock_account(accounts_dir, account_ref)?;
        if record_path(accounts_dir, account_ref)?.exists() {
            continue;
        }
        if let Some(receipt) = read_optional_receipt(&receipt_path(accounts_dir, account_ref)?)? {
            anyhow::ensure!(
                receipt.account_ref == account_ref,
                "credential removal receipt identity mismatch"
            );
        }
        fingerprint::remove_if_exists(accounts_dir, account_ref)?;
    }
    Ok(())
}

fn read_record(path: &Path) -> Result<VaultRecord> {
    read_json_limited(path, 1024 * 1024, "credential record")
}

fn write_record_atomically(accounts_dir: &Path, path: &Path, record: &VaultRecord) -> Result<()> {
    write_json_atomically(accounts_dir, path, record)
}

fn lock_account(accounts_dir: &Path, account_ref: &str) -> Result<File> {
    validate_account_ref(account_ref)?;
    let lock_path = accounts_dir.join(format!("{account_ref}.lock"));
    let lock_file = open_private_file(&lock_path)?;
    lock_file
        .lock_exclusive()
        .context("locking credential record")?;
    Ok(lock_file)
}

#[cfg(test)]
#[path = "vault_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "fingerprint_vault_tests.rs"]
mod fingerprint_tests;
