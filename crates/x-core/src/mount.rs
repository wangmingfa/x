//! Mounted filesystems: list, inspect, mount and unmount.
//!
//! Listing reuses the disk capability's view where possible; `mount` /
//! `unmount` go through the platform's own tool and are the kind of operation
//! that usually needs privileges — the adapters report the requirement
//! instead of a bare failure.

use serde::{Deserialize, Serialize};

/// One mounted filesystem (or one the platform knows how to mount).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MountInfo {
    /// Device / source (`/dev/sda1`, `//server/share`, `C:`).
    pub source: String,
    /// Mount point.
    pub target: String,
    /// Filesystem type (`ext4`, `ntfs`, `apfs`, ...).
    pub fs_type: String,
    /// Read-only.
    pub readonly: bool,
    /// Size in bytes, when reported.
    pub total_bytes: Option<u64>,
    /// Used bytes, when reported.
    pub used_bytes: Option<u64>,
}

/// Mount management.
pub trait MountManager: Send + Sync {
    /// Everything currently mounted.
    fn list(&self) -> crate::error::Result<Vec<MountInfo>>;

    /// One mount point in detail.
    fn info(&self, target: &str) -> crate::error::Result<MountInfo> {
        self.list()?
            .into_iter()
            .find(|m| m.target.eq_ignore_ascii_case(target))
            .ok_or_else(|| crate::Error::not_found(format!("not mounted: {target}")))
    }

    /// Mount `source` at `target` (best-effort platform tool).
    fn mount(&self, source: &str, target: &str) -> crate::error::Result<()>;

    /// Unmount whatever is at `target`.
    fn unmount(&self, target: &str) -> crate::error::Result<()>;
}
