//! Unified disk model.

use serde::{Deserialize, Serialize};

/// A mounted filesystem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskInfo {
    /// Mount point.
    pub mount_point: String,
    /// Device name when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Filesystem type, e.g. `apfs`, `ext4`, `ntfs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_system: Option<String>,
    /// Whether the filesystem is read only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    /// Total bytes.
    pub total_bytes: u64,
    /// Available bytes.
    pub available_bytes: u64,
    /// Usage percentage.
    pub percent: f32,
}

impl DiskInfo {
    /// Bytes in use.
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.available_bytes)
    }
}

/// Disk capability.
pub trait DiskManager: Send + Sync {
    /// List mounted filesystems.
    fn list(&self) -> crate::error::Result<Vec<DiskInfo>>;

    /// Usage of the filesystem containing the current working directory.
    fn current(&self) -> crate::error::Result<DiskInfo> {
        self.list()?
            .into_iter()
            .max_by_key(|d| d.mount_point.len())
            .ok_or_else(|| crate::error::Error::not_found("no mounted filesystem found"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_bytes_is_saturating() {
        let disk = DiskInfo {
            mount_point: "/".into(),
            name: None,
            file_system: Some("apfs".into()),
            read_only: None,
            total_bytes: 100,
            available_bytes: 40,
            percent: 60.0,
        };
        assert_eq!(disk.used_bytes(), 60);
        assert_eq!(
            DiskInfo {
                available_bytes: 200,
                ..disk.clone()
            }
            .used_bytes(),
            0
        );
    }
}
