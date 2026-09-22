//! Coherence evidence for an already validated private ledger, checked under its file lock.
use anyhow::{Context, Result};
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StateStamp {
    device: u64,
    inode: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl StateStamp {
    #[cfg(unix)]
    pub(crate) fn read(path: &Path) -> Result<Option<Self>> {
        use std::os::unix::fs::MetadataExt;
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("checking response identity state"),
        };
        if !metadata.is_file()
            || metadata.mode() & 0o7777 != 0o600
            || metadata.len() > crate::request_state_types::MAX_REQUEST_STATE_BYTES
        {
            return Ok(None);
        }
        Ok(Some(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }))
    }

    // Without a strong change stamp, keep the original transaction for every event.
    #[cfg(not(unix))]
    pub(crate) fn read(_path: &Path) -> Result<Option<Self>> {
        Ok(None)
    }
}
