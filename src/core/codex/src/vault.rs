use anyhow::Context;
use anyhow::Result;
use chrono::DateTime;
use chrono::Utc;
use fs2::FileExt;
use serde::Deserialize;
use serde::Serialize;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use uuid::Uuid;

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
}

#[derive(Clone)]
pub struct Vault {
    state_dir: PathBuf,
    accounts_dir: PathBuf,
}

pub struct InstanceLock {
    file: File,
}

pub struct LockedRecord {
    lock_file: File,
    record_path: PathBuf,
    accounts_dir: PathBuf,
    pub record: VaultRecord,
}

impl Vault {
    pub fn open(state_dir: PathBuf) -> Result<Self> {
        let accounts_dir = state_dir.join("accounts");
        std::fs::create_dir_all(&accounts_dir).context("creating credential vault")?;
        set_private_directory(&state_dir)?;
        set_private_directory(&accounts_dir)?;
        Ok(Self {
            state_dir,
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
    ) -> Result<CredentialMetadata> {
        let record = new_record(material, upstream_url);
        self.create_record(record).await
    }

    pub async fn create_api_key(
        &self,
        api_key: String,
        upstream_url: String,
    ) -> Result<CredentialMetadata> {
        let record = new_record(CredentialMaterial::OpenAiApiKey { api_key }, upstream_url);
        self.create_record(record).await
    }

    async fn create_record(&self, record: VaultRecord) -> Result<CredentialMetadata> {
        let metadata = record.metadata();
        let accounts_dir = self.accounts_dir.clone();
        tokio::task::spawn_blocking(move || {
            let path = record_path(&accounts_dir, &record.account_ref)?;
            if path.exists() {
                anyhow::bail!("credential reference collision");
            }
            write_record_atomically(&accounts_dir, &path, &record)
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
            let record = read_record(&record_path)?;
            Ok(LockedRecord {
                lock_file,
                record_path,
                accounts_dir,
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
}

impl LockedRecord {
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

fn read_record(path: &Path) -> Result<VaultRecord> {
    read_json_limited(path, 1024 * 1024, "credential record")
}

fn read_optional_receipt(path: &Path) -> Result<Option<RemovalReceipt>> {
    match read_json_limited(path, 64 * 1024, "credential removal receipt") {
        Ok(receipt) => Ok(Some(receipt)),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|source| source.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn read_json_limited<T: serde::de::DeserializeOwned>(
    path: &Path,
    maximum: u64,
    description: &'static str,
) -> Result<T> {
    let file = File::open(path).context("opening credential record")?;
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {description}"))?;
    anyhow::ensure!(bytes.len() as u64 <= maximum, "{description} is too large");
    serde_json::from_slice(&bytes).with_context(|| format!("decoding {description}"))
}

fn write_record_atomically(accounts_dir: &Path, path: &Path, record: &VaultRecord) -> Result<()> {
    write_json_atomically(accounts_dir, path, record)
}

fn write_json_atomically<T: Serialize>(accounts_dir: &Path, path: &Path, value: &T) -> Result<()> {
    let temp_path = accounts_dir.join(format!(".credential-{}.tmp", Uuid::new_v4().simple()));
    let bytes = serde_json::to_vec(value).context("encoding private vault state")?;
    let mut file = open_private_file(&temp_path)?;
    file.set_len(0).context("truncating credential temp file")?;
    file.write_all(&bytes)
        .context("writing credential temp file")?;
    file.sync_all().context("syncing credential temp file")?;
    std::fs::rename(&temp_path, path).context("replacing credential record")?;
    sync_directory(accounts_dir);
    Ok(())
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

fn receipt_path(accounts_dir: &Path, account_ref: &str) -> Result<PathBuf> {
    validate_account_ref(account_ref)?;
    Ok(accounts_dir.join(format!("{account_ref}.removal.json")))
}

fn complete_removal_locked(
    accounts_dir: &Path,
    record_path: &Path,
    account_ref: &str,
    requested_kind: RemovalKind,
) -> Result<RemovalReceipt> {
    let receipt_path = receipt_path(accounts_dir, account_ref)?;
    let receipt = match read_optional_receipt(&receipt_path)? {
        Some(existing) => {
            anyhow::ensure!(
                existing.account_ref == account_ref,
                "credential removal receipt identity mismatch"
            );
            anyhow::ensure!(
                existing.kind == requested_kind
                    || (existing.kind == RemovalKind::OAuthRevoked
                        && requested_kind == RemovalKind::ServiceOnly),
                "credential was removed without an upstream OAuth revoke"
            );
            existing
        }
        None => {
            let receipt = RemovalReceipt {
                account_ref: account_ref.to_string(),
                kind: requested_kind,
                completed_at: Utc::now(),
            };
            write_json_atomically(accounts_dir, &receipt_path, &receipt)?;
            receipt
        }
    };
    match std::fs::remove_file(record_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("removing credential record"),
    }
    sync_directory(accounts_dir);
    Ok(receipt)
}

fn sync_directory(accounts_dir: &Path) {
    if let Ok(directory) = File::open(accounts_dir) {
        let _ = directory.sync_all();
    }
}

fn open_private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path).context("opening private state file")?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .context("setting private file permissions")?;
    Ok(file)
}

fn set_private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .context("setting private directory permissions")?;
    Ok(())
}

#[cfg(test)]
#[path = "vault_tests.rs"]
mod tests;
