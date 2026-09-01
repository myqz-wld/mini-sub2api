use anyhow::Context;
use anyhow::Result;
use chrono::Utc;
use fs2::FileExt;
use std::collections::BTreeSet;
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

use crate::request_state_editor::RequestStateEditor;
use crate::request_state_lookup::LookupKeyFactory;
use crate::request_state_types::INITIAL_REQUEST_STATE_REVISION;
use crate::request_state_types::MAX_REQUEST_STATE_BYTES;
use crate::request_state_types::PersistedRequestState;
use crate::request_state_types::validate_account_ref;
use crate::vault::CredentialMaterial;
use crate::vault::VaultRecord;
use crate::vault_io::open_private_file;
use crate::vault_io::read_json_limited;
use crate::vault_io::sync_directory;

const RECORD_MAXIMUM_BYTES: u64 = 1024 * 1024;
const STATE_SUFFIX: &str = ".request-state.json";
const LOCK_SUFFIX: &str = ".request-state.lock";

#[derive(Clone)]
pub(crate) struct RequestStateStore {
    accounts_dir: PathBuf,
}

impl RequestStateStore {
    pub(crate) fn new(accounts_dir: PathBuf) -> Self {
        Self { accounts_dir }
    }

    pub(crate) async fn edit<R, F>(
        &self,
        account_namespace: &str,
        owner_account_ref: &str,
        downstream_scope: &str,
        operation: F,
    ) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut RequestStateEditor<'_>) -> Result<R> + Send + 'static,
    {
        self.edit_at(
            account_namespace,
            owner_account_ref,
            downstream_scope,
            Utc::now().timestamp_millis(),
            operation,
        )
        .await
    }

    pub(crate) async fn edit_at<R, F>(
        &self,
        account_namespace: &str,
        owner_account_ref: &str,
        downstream_scope: &str,
        now_unix_ms: i64,
        operation: F,
    ) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut RequestStateEditor<'_>) -> Result<R> + Send + 'static,
    {
        validate_account_ref(owner_account_ref)?;
        anyhow::ensure!(
            !account_namespace.is_empty(),
            "empty request state namespace"
        );
        anyhow::ensure!(!downstream_scope.is_empty(), "empty downstream scope");
        anyhow::ensure!(now_unix_ms >= 0, "invalid request state time");
        let accounts_dir = self.accounts_dir.clone();
        let account_namespace = account_namespace.to_string();
        let owner_account_ref = owner_account_ref.to_string();
        let downstream_scope = downstream_scope.to_string();
        tokio::task::spawn_blocking(move || {
            edit_locked(
                &accounts_dir,
                &account_namespace,
                &owner_account_ref,
                &downstream_scope,
                now_unix_ms,
                operation,
            )
        })
        .await
        .context("request state edit task failed")?
    }

    pub(crate) fn register_owner_if_exists(
        &self,
        account_namespace: &str,
        owner_account_ref: &str,
    ) -> Result<()> {
        validate_account_ref(owner_account_ref)?;
        let state_ref = LookupKeyFactory::account_state_ref(account_namespace);
        let _lock = lock_state(&self.accounts_dir, &state_ref)?;
        let path = state_path(&self.accounts_dir, &state_ref);
        let Some(mut state) = read_optional_state(&path)? else {
            return Ok(());
        };
        if state.owners.insert(owner_account_ref.to_string()) {
            state.revision = next_revision(state.revision)?;
            state.validate()?;
            write_state(&self.accounts_dir, &path, &state)?;
        }
        Ok(())
    }

    pub(crate) fn remove_owner(
        &self,
        account_namespace: &str,
        owner_account_ref: &str,
    ) -> Result<()> {
        validate_account_ref(owner_account_ref)?;
        let state_ref = LookupKeyFactory::account_state_ref(account_namespace);
        let _lock = lock_state(&self.accounts_dir, &state_ref)?;
        let path = state_path(&self.accounts_dir, &state_ref);
        let Some(mut state) = read_optional_state(&path)? else {
            return Ok(());
        };
        let mut owners = owners_for_namespace(&self.accounts_dir, account_namespace)?;
        owners.remove(owner_account_ref);
        state.owners = owners;
        if state.owners.is_empty() {
            remove_file_if_exists(&self.accounts_dir, &path)?;
            return Ok(());
        }
        state.revision = next_revision(state.revision)?;
        state.validate()?;
        write_state(&self.accounts_dir, &path, &state)
    }

    #[cfg(test)]
    pub(crate) fn state_path_for_test(&self, account_namespace: &str) -> PathBuf {
        state_path(
            &self.accounts_dir,
            &LookupKeyFactory::account_state_ref(account_namespace),
        )
    }
}

pub(crate) fn cleanup_orphan_request_states(accounts_dir: &Path) -> Result<()> {
    let active = active_state_refs(accounts_dir)?;
    for entry in std::fs::read_dir(accounts_dir).context("scanning request state files")? {
        let entry = entry.context("reading request state entry")?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(state_ref) = name.strip_suffix(STATE_SUFFIX) else {
            continue;
        };
        if !valid_state_ref(state_ref) || active.contains(state_ref) {
            continue;
        }
        let _lock = lock_state(accounts_dir, state_ref)?;
        remove_file_if_exists(accounts_dir, &entry.path())?;
    }
    Ok(())
}

fn edit_locked<R, F>(
    accounts_dir: &Path,
    account_namespace: &str,
    owner_account_ref: &str,
    downstream_scope: &str,
    now_unix_ms: i64,
    operation: F,
) -> Result<R>
where
    F: FnOnce(&mut RequestStateEditor<'_>) -> Result<R>,
{
    let state_ref = LookupKeyFactory::account_state_ref(account_namespace);
    let _lock = lock_state(accounts_dir, &state_ref)?;
    let path = state_path(accounts_dir, &state_ref);
    let existing = read_optional_state(&path)?;
    let created = existing.is_none();
    let mut state = match existing {
        Some(state) => state,
        None => {
            let mut owners = owners_for_namespace(accounts_dir, account_namespace)?;
            owners.insert(owner_account_ref.to_string());
            PersistedRequestState::new(owners)
        }
    };
    state.validate()?;
    let keys = LookupKeyFactory::new(account_namespace, downstream_scope);
    let day = now_unix_ms / 86_400_000;
    let mut editor =
        RequestStateEditor::new(&mut state, keys, owner_account_ref, day, now_unix_ms)?;
    let output = operation(&mut editor)?;
    let summary = editor.finish();
    let mut changed = summary.changed | state.prune(day, &summary.protected)?;
    let mut bytes = serde_json::to_vec(&state).context("encoding request state")?;
    while bytes.len() as u64 > MAX_REQUEST_STATE_BYTES {
        anyhow::ensure!(
            state.evict_one(&summary.protected),
            "request state cannot fit within the size limit"
        );
        changed = true;
        bytes = serde_json::to_vec(&state).context("encoding pruned request state")?;
    }
    if created || changed {
        if !created {
            state.revision = next_revision(state.revision)?;
            bytes = serde_json::to_vec(&state).context("encoding revised request state")?;
        }
        state.validate()?;
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_REQUEST_STATE_BYTES,
            "request state is too large"
        );
        write_bytes_atomically(accounts_dir, &path, &bytes)?;
    }
    Ok(output)
}

fn read_optional_state(path: &Path) -> Result<Option<PersistedRequestState>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading request state metadata"),
    };
    anyhow::ensure!(
        metadata.file_type().is_file(),
        "request state is not a regular file"
    );
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .context("setting request state permissions")?;
    let file = File::open(path).context("opening request state")?;
    let mut bytes = Vec::new();
    file.take(MAX_REQUEST_STATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("reading request state")?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_REQUEST_STATE_BYTES,
        "request state is too large"
    );
    let state = serde_json::from_slice::<PersistedRequestState>(&bytes)
        .context("decoding request state")?;
    state.validate()?;
    Ok(Some(state))
}

fn write_state(accounts_dir: &Path, path: &Path, state: &PersistedRequestState) -> Result<()> {
    let bytes = serde_json::to_vec(state).context("encoding request state")?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_REQUEST_STATE_BYTES,
        "request state is too large"
    );
    write_bytes_atomically(accounts_dir, path, &bytes)
}

fn write_bytes_atomically(accounts_dir: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = accounts_dir.join(format!(".request-state-{}.tmp", Uuid::new_v4().simple()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temp)
            .context("creating request state temp file")?;
        file.write_all(bytes)
            .context("writing request state temp file")?;
        file.sync_all().context("syncing request state temp file")?;
        std::fs::rename(&temp, path).context("replacing request state")?;
        sync_directory(accounts_dir);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn owners_for_namespace(accounts_dir: &Path, namespace: &str) -> Result<BTreeSet<String>> {
    let mut owners = BTreeSet::new();
    for entry in std::fs::read_dir(accounts_dir).context("scanning credential owners")? {
        let entry = entry.context("reading credential owner entry")?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(account_ref) = name.strip_suffix(".json") else {
            continue;
        };
        if validate_account_ref(account_ref).is_err() {
            continue;
        }
        let record = match read_json_limited::<VaultRecord>(
            &entry.path(),
            RECORD_MAXIMUM_BYTES,
            "credential owner record",
        ) {
            Ok(record) => record,
            Err(error) if is_not_found(&error) => continue,
            Err(error) => return Err(error),
        };
        if matches!(
            &record.material,
            CredentialMaterial::CodexOAuth { account_id, .. } if account_id == namespace
        ) {
            owners.insert(record.account_ref);
        }
    }
    Ok(owners)
}

fn active_state_refs(accounts_dir: &Path) -> Result<BTreeSet<String>> {
    let mut active = BTreeSet::new();
    for entry in std::fs::read_dir(accounts_dir).context("scanning credential namespaces")? {
        let entry = entry.context("reading credential namespace entry")?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(account_ref) = name.strip_suffix(".json") else {
            continue;
        };
        if validate_account_ref(account_ref).is_err() {
            continue;
        }
        let record = match read_json_limited::<VaultRecord>(
            &entry.path(),
            RECORD_MAXIMUM_BYTES,
            "credential namespace record",
        ) {
            Ok(record) => record,
            Err(error) if is_not_found(&error) => continue,
            Err(error) => return Err(error),
        };
        if let CredentialMaterial::CodexOAuth { account_id, .. } = record.material {
            active.insert(LookupKeyFactory::account_state_ref(&account_id));
        }
    }
    Ok(active)
}

fn state_path(accounts_dir: &Path, state_ref: &str) -> PathBuf {
    accounts_dir.join(format!("{state_ref}{STATE_SUFFIX}"))
}

fn lock_state(accounts_dir: &Path, state_ref: &str) -> Result<File> {
    anyhow::ensure!(
        valid_state_ref(state_ref),
        "invalid request state reference"
    );
    let lock = open_private_file(&accounts_dir.join(format!("{state_ref}{LOCK_SUFFIX}")))?;
    lock.lock_exclusive().context("locking request state")?;
    Ok(lock)
}

fn valid_state_ref(state_ref: &str) -> bool {
    state_ref.strip_prefix("rs_").is_some_and(|encoded| {
        encoded.len() == 43
            && encoded
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    })
}

fn next_revision(revision: u64) -> Result<u64> {
    anyhow::ensure!(
        revision >= INITIAL_REQUEST_STATE_REVISION,
        "invalid request state revision"
    );
    revision
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("request state revision exhausted"))
}

fn remove_file_if_exists(accounts_dir: &Path, path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => sync_directory(accounts_dir),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("removing request state"),
    }
    Ok(())
}

fn is_not_found(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|source| source.downcast_ref::<std::io::Error>())
        .any(|source| source.kind() == std::io::ErrorKind::NotFound)
}

#[cfg(test)]
#[path = "request_state_store_tests.rs"]
mod tests;
