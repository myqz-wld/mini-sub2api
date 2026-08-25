use anyhow::Context;
use anyhow::Result;
use clap::ValueEnum;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
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

const FINGERPRINT_VERSION: u32 = 1;
const INITIAL_REVISION: u64 = 1;
const MAX_FINGERPRINT_BYTES: u64 = 64 * 1024;
const FINGERPRINT_SUFFIX: &str = ".fingerprint.json";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum FingerprintMode {
    Off,
    #[default]
    Device,
}

#[derive(Clone, Eq, PartialEq)]
pub struct FingerprintSnapshot {
    version: u32,
    revision: u64,
    mode: FingerprintMode,
}

impl fmt::Debug for FingerprintSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FingerprintSnapshot")
            .field("version", &self.version)
            .field("revision", &self.revision)
            .field("mode", &self.mode)
            .finish()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedFingerprint {
    version: u32,
    revision: u64,
    mode: FingerprintMode,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintMetadata {
    pub account_ref: String,
    pub mode: FingerprintMode,
    pub revision: u64,
}

impl FingerprintSnapshot {
    fn new(mode: FingerprintMode) -> Self {
        Self {
            version: FINGERPRINT_VERSION,
            revision: INITIAL_REVISION,
            mode,
        }
    }

    pub fn mode(&self) -> FingerprintMode {
        self.mode
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn metadata(&self, account_ref: &str) -> FingerprintMetadata {
        FingerprintMetadata {
            account_ref: account_ref.to_string(),
            mode: self.mode,
            revision: self.revision,
        }
    }

    fn persisted(&self) -> PersistedFingerprint {
        PersistedFingerprint {
            version: self.version,
            revision: self.revision,
            mode: self.mode,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(mode: FingerprintMode, revision: u64) -> Self {
        Self {
            version: FINGERPRINT_VERSION,
            revision,
            mode,
        }
    }
}

pub(crate) fn sidecar_path(accounts_dir: &Path, account_ref: &str) -> PathBuf {
    accounts_dir.join(format!("{account_ref}{FINGERPRINT_SUFFIX}"))
}

pub(crate) fn account_ref_from_sidecar_name(name: &str) -> Option<&str> {
    name.strip_suffix(FINGERPRINT_SUFFIX)
}

pub(crate) fn create(
    accounts_dir: &Path,
    account_ref: &str,
    mode: FingerprintMode,
) -> Result<FingerprintSnapshot> {
    let path = sidecar_path(accounts_dir, account_ref);
    anyhow::ensure!(!path.exists(), "credential fingerprint already exists");
    let snapshot = FingerprintSnapshot::new(mode);
    write_atomically(accounts_dir, &path, &snapshot.persisted())?;
    Ok(snapshot)
}

pub(crate) fn load_or_materialize(
    accounts_dir: &Path,
    account_ref: &str,
) -> Result<FingerprintSnapshot> {
    let path = sidecar_path(accounts_dir, account_ref);
    match read(&path) {
        Ok(snapshot) => Ok(snapshot),
        Err(error) if is_not_found(&error) => {
            let snapshot = FingerprintSnapshot::new(FingerprintMode::Device);
            write_atomically(accounts_dir, &path, &snapshot.persisted())?;
            Ok(snapshot)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn update_mode(
    accounts_dir: &Path,
    account_ref: &str,
    snapshot: &FingerprintSnapshot,
    mode: FingerprintMode,
) -> Result<FingerprintSnapshot> {
    if snapshot.mode == mode {
        return Ok(snapshot.clone());
    }
    let revision = snapshot
        .revision
        .checked_add(1)
        .context("credential fingerprint revision exhausted")?;
    let updated = FingerprintSnapshot {
        version: snapshot.version,
        revision,
        mode,
    };
    write_atomically(
        accounts_dir,
        &sidecar_path(accounts_dir, account_ref),
        &updated.persisted(),
    )?;
    Ok(updated)
}

pub(crate) fn remove_if_exists(accounts_dir: &Path, account_ref: &str) -> Result<bool> {
    let path = sidecar_path(accounts_dir, account_ref);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            sync_directory(accounts_dir);
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("removing credential fingerprint"),
    }
}

fn read(path: &Path) -> Result<FingerprintSnapshot> {
    let metadata =
        std::fs::symlink_metadata(path).context("reading credential fingerprint metadata")?;
    anyhow::ensure!(
        metadata.file_type().is_file(),
        "credential fingerprint is not a regular file"
    );
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .context("setting credential fingerprint permissions")?;

    let file = File::open(path).context("opening credential fingerprint")?;
    let mut bytes = Vec::new();
    file.take(MAX_FINGERPRINT_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("reading credential fingerprint")?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_FINGERPRINT_BYTES,
        "credential fingerprint is too large"
    );
    let persisted: PersistedFingerprint =
        serde_json::from_slice(&bytes).context("decoding credential fingerprint")?;
    validate(persisted)
}

fn validate(persisted: PersistedFingerprint) -> Result<FingerprintSnapshot> {
    anyhow::ensure!(
        persisted.version == FINGERPRINT_VERSION,
        "unsupported credential fingerprint version"
    );
    anyhow::ensure!(
        persisted.revision >= INITIAL_REVISION,
        "invalid credential fingerprint revision"
    );
    Ok(FingerprintSnapshot {
        version: persisted.version,
        revision: persisted.revision,
        mode: persisted.mode,
    })
}

fn write_atomically<T: Serialize>(accounts_dir: &Path, path: &Path, value: &T) -> Result<()> {
    let temp_path = accounts_dir.join(format!(".fingerprint-{}.tmp", Uuid::new_v4().simple()));
    let result = (|| {
        let bytes = serde_json::to_vec(value).context("encoding credential fingerprint")?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temp_path)
            .context("opening credential fingerprint temp file")?;
        file.write_all(&bytes)
            .context("writing credential fingerprint temp file")?;
        file.sync_all()
            .context("syncing credential fingerprint temp file")?;
        std::fs::rename(&temp_path, path).context("replacing credential fingerprint")?;
        sync_directory(accounts_dir);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn sync_directory(accounts_dir: &Path) {
    if let Ok(directory) = File::open(accounts_dir) {
        let _ = directory.sync_all();
    }
}

fn is_not_found(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|source| source.downcast_ref::<std::io::Error>())
        .any(|source| source.kind() == std::io::ErrorKind::NotFound)
}
