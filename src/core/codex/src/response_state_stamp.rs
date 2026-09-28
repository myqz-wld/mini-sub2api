//! Coherence evidence for an already validated private ledger, checked under its file lock.
use anyhow::{Context, Result};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// File timestamps can stay equal across several same-length writes. Do not trust
// equality until the recorded change is outside the timestamp update window.
const SETTLE_TIME: Duration = Duration::from_secs(2);

fn settled(timestamp: (i64, i64), now: SystemTime) -> bool {
    let (Ok(seconds), Ok(nanos)) = (u64::try_from(timestamp.0), u32::try_from(timestamp.1)) else {
        return false;
    };
    if nanos >= 1_000_000_000 {
        return false;
    }
    UNIX_EPOCH
        .checked_add(Duration::new(seconds, nanos))
        .and_then(|changed| now.duration_since(changed).ok())
        .is_some_and(|age| age >= SETTLE_TIME)
}

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
        let modified = (metadata.mtime(), metadata.mtime_nsec());
        let changed = (metadata.ctime(), metadata.ctime_nsec());
        let now = SystemTime::now();
        if !settled(modified, now) || !settled(changed, now) {
            return Ok(None);
        }
        Ok(Some(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            modified,
            changed,
        }))
    }

    // Without a strong change stamp, keep the original transaction for every event.
    #[cfg(not(unix))]
    pub(crate) fn read(_path: &Path) -> Result<Option<Self>> {
        Ok(None)
    }

    #[cfg(test)]
    pub(crate) fn wait_until_cacheable(path: &Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while Self::read(path).unwrap().is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "state stamp did not settle"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_coarse_future_and_invalid_timestamps_cannot_validate_cache() {
        let now = UNIX_EPOCH + Duration::new(100, 500_000_000);
        for timestamp in [
            (100, 0),
            (99, 0),
            (98, 500_000_001),
            (101, 0),
            (-1, 0),
            (0, -1),
            (0, 1_000_000_000),
        ] {
            assert!(!settled(timestamp, now), "{timestamp:?}");
        }
        assert!(settled((98, 500_000_000), now));
        assert!(settled((98, 0), now));
    }

    #[test]
    fn clock_rollback_disables_previously_settled_evidence() {
        let timestamp = (100, 0);
        assert!(settled(timestamp, UNIX_EPOCH + Duration::from_secs(103)));
        assert!(!settled(timestamp, UNIX_EPOCH + Duration::from_secs(101)));
        assert!(!settled(timestamp, UNIX_EPOCH + Duration::from_secs(99)));
    }

    #[cfg(unix)]
    #[test]
    fn backdated_mtime_does_not_hide_a_recent_change_time() {
        use std::os::unix::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("synthetic-state");
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(100))
            .unwrap();
        assert!(StateStamp::read(&path).unwrap().is_none());
    }
}
