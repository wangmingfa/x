//! Disk adapter built on `sysinfo`, shared by every platform.

use sysinfo::Disks;
use x_core::disk::{DiskInfo, DiskManager};
use x_core::error::Result;
use x_core::system::percent;

/// Lists mounted filesystems.
#[derive(Debug, Default)]
pub struct SysinfoDisk;

impl SysinfoDisk {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }
}

impl DiskManager for SysinfoDisk {
    fn list(&self) -> Result<Vec<DiskInfo>> {
        let disks = Disks::new_with_refreshed_list();
        let mut rows: Vec<DiskInfo> = disks
            .list()
            .iter()
            .filter(|disk| disk.total_space() > 0)
            .map(|disk| {
                let total = disk.total_space();
                let available = disk.available_space();
                DiskInfo {
                    mount_point: disk.mount_point().to_string_lossy().into_owned(),
                    name: Some(disk.name().to_string_lossy().into_owned())
                        .filter(|name| !name.is_empty()),
                    file_system: Some(disk.file_system().to_string_lossy().into_owned()),
                    read_only: Some(disk.is_read_only()),
                    label: None,
                    volume_uuid: None,
                    partition_uuid: None,
                    device_model: None,
                    device_serial: None,
                    media_type: None,
                    total_bytes: total,
                    available_bytes: available,
                    percent: percent(total.saturating_sub(available), total),
                }
            })
            .collect();

        // Apple firmlinks expose the same volume twice; keep the shortest path.
        rows.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
        rows.dedup_by(|a, b| a.mount_point == b.mount_point && a.total_bytes == b.total_bytes);
        Ok(rows)
    }
}

/// Trait object helper.
pub fn as_manager() -> std::sync::Arc<dyn DiskManager> {
    std::sync::Arc::new(SysinfoDisk::new())
}

/// Default adapter.
pub fn manager() -> std::sync::Arc<dyn DiskManager> {
    as_manager()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_filesystem_is_listed() {
        let rows = SysinfoDisk::new().list().expect("list");
        assert!(!rows.is_empty(), "at least one filesystem must be mounted");
        assert!(rows.iter().all(|d| d.total_bytes > 0));
        assert!(rows.iter().all(|d| (0.0..=100.0).contains(&d.percent)));
    }

    #[test]
    fn current_returns_the_deepest_mount() {
        let current = SysinfoDisk::new().current().expect("current");
        assert!(current.total_bytes > 0);
    }
}
