use anyhow::Context;
use anyhow::Result;
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
use uuid::Uuid;

pub(crate) fn read_json_limited<T: serde::de::DeserializeOwned>(
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

pub(crate) fn write_json_atomically<T: Serialize>(
    accounts_dir: &Path,
    path: &Path,
    value: &T,
) -> Result<()> {
    let temp_path = accounts_dir.join(format!(".credential-{}.tmp", Uuid::new_v4().simple()));
    let result = (|| {
        let bytes = serde_json::to_vec(value).context("encoding private vault state")?;
        let mut file = open_private_file(&temp_path)?;
        file.set_len(0).context("truncating credential temp file")?;
        file.write_all(&bytes)
            .context("writing credential temp file")?;
        file.sync_all().context("syncing credential temp file")?;
        std::fs::rename(&temp_path, path).context("replacing credential record")?;
        sync_directory(accounts_dir);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

pub(crate) fn sync_directory(directory: &Path) {
    if let Ok(directory) = File::open(directory) {
        let _ = directory.sync_all();
    }
}

pub(crate) fn open_private_file(path: &Path) -> Result<File> {
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

pub(crate) fn set_private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .context("setting private directory permissions")?;
    Ok(())
}
