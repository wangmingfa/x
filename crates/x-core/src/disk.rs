//! Unified disk model.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Storage class of the medium backing a filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    /// Spinning magnetic disk.
    Hdd,
    /// SATA / AHCI / SCSI solid state device.
    Ssd,
    /// NVMe device.
    Nvme,
    /// Removable or USB media.
    Removable,
    /// Network mount (NFS, SMB, ...).
    Network,
    /// Virtual or stacked device (dm, loop, container overlay).
    Virtual,
}

impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            MediaType::Hdd => "hdd",
            MediaType::Ssd => "ssd",
            MediaType::Nvme => "nvme",
            MediaType::Removable => "removable",
            MediaType::Network => "network",
            MediaType::Virtual => "virtual",
        };
        f.write_str(name)
    }
}

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
    /// Volume label when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Filesystem UUID of the mounted volume when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_uuid: Option<String>,
    /// Partition UUID (GPT partition GUID) when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition_uuid: Option<String>,
    /// Model of the disk the partition lives on when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,
    /// Serial number of the disk the partition lives on when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_serial: Option<String>,
    /// Storage medium class when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<MediaType>,
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

/// Cumulative block-device I/O counters, as the kernel reports them.
///
/// Cumulative since boot, per whole disk — the CLI diffs two samples into
/// rates the same way `x net top` does. Devices the platform cannot read
/// are simply absent; a failed read is an error, never zeros.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskIo {
    /// Whole-disk device name as the platform spells it (`sda`, `disk0`,
    /// `PhysicalDrive0`).
    pub device: String,
    /// Total bytes read, cumulative.
    pub read_bytes: u64,
    /// Total bytes written, cumulative.
    pub write_bytes: u64,
    /// Total read operations, cumulative.
    pub read_ops: u64,
    /// Total write operations, cumulative.
    pub write_ops: u64,
}

/// Interval rates derived from two [`DiskIo`] samples of the same device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskIoRates {
    /// Whole-disk device name.
    pub device: String,
    /// Bytes read per second over the interval.
    pub read_bytes_per_sec: f64,
    /// Bytes written per second over the interval.
    pub write_bytes_per_sec: f64,
    /// Read operations per second over the interval.
    pub read_ops_per_sec: f64,
    /// Write operations per second over the interval.
    pub write_ops_per_sec: f64,
}

/// Rate derivation from two cumulative samples.
///
/// A counter that went backwards (device replaced, counter reset) yields
/// zeros for that direction instead of a wrapped garbage number; callers
/// see `previous` returning so they can note the reset rather than rate it.
pub fn diff_disk_io(previous: &DiskIo, current: &DiskIo, seconds: f64) -> DiskIoRates {
    fn per_sec(after: u64, before: u64, seconds: f64) -> f64 {
        if seconds <= 0.0 || after < before {
            return 0.0;
        }
        (after - before) as f64 / seconds
    }
    DiskIoRates {
        device: current.device.clone(),
        read_bytes_per_sec: per_sec(current.read_bytes, previous.read_bytes, seconds),
        write_bytes_per_sec: per_sec(current.write_bytes, previous.write_bytes, seconds),
        read_ops_per_sec: per_sec(current.read_ops, previous.read_ops, seconds),
        write_ops_per_sec: per_sec(current.write_ops, previous.write_ops, seconds),
    }
}

/// Disk capability.
pub trait DiskManager: Send + Sync {
    /// List mounted filesystems.
    fn list(&self) -> crate::error::Result<Vec<DiskInfo>>;

    /// Cumulative I/O counters of every whole disk the platform can read.
    ///
    /// Defaults to "no source on this platform": callers treat the error as
    /// an honest absence, the same way `x net top` treats a missing L3.
    fn io(&self) -> crate::error::Result<Vec<DiskIo>> {
        let _ = self;
        Err(crate::error::Error::unsupported(
            "block-device I/O counters are not available on this platform",
        ))
    }

    /// Usage of the filesystem containing the current working directory.
    fn current(&self) -> crate::error::Result<DiskInfo> {
        self.list()?
            .into_iter()
            .max_by_key(|d| d.mount_point.len())
            .ok_or_else(|| crate::error::Error::not_found("no mounted filesystem found"))
    }
}

/// One directory in a disk-usage walk, with all descendant sizes aggregated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirUsage {
    /// Directory path, starting from the walk root.
    pub path: PathBuf,
    /// Depth below the walk root (root itself is `0`).
    pub depth: usize,
    /// Bytes used by the directory and everything inside it.
    pub total_bytes: u64,
    /// Regular files in the whole subtree.
    pub files: u64,
    /// Directories in the whole subtree (not counting the directory itself).
    pub dirs: u64,
    /// Entries or directories that could not be read; the totals are honest
    /// but incomplete by however many this reports.
    pub unreadable: u64,
}

/// Aggregated size of one directory while walking.
struct WalkNode {
    path: PathBuf,
    parent: Option<usize>,
    depth: usize,
    total_bytes: u64,
    files: u64,
    dirs: u64,
    unreadable: u64,
}

/// Walk `root` like `du`: every directory is measured including all
/// descendants, symlinks are not followed, unreadable entries are counted
/// instead of aborting the walk.
///
/// `max_depth` only limits which nodes are returned (du `-d` semantics);
/// sizes are always computed from the complete tree. Results are sorted by
/// `total_bytes`, largest first, and the root is always included.
pub fn walk_directory(root: &Path, max_depth: Option<usize>) -> Vec<DirUsage> {
    let mut nodes = vec![WalkNode {
        path: root.to_path_buf(),
        parent: None,
        depth: 0,
        total_bytes: 0,
        files: 0,
        dirs: 0,
        unreadable: 0,
    }];
    let mut stack = vec![0usize];

    while let Some(idx) = stack.pop() {
        let entries = match std::fs::read_dir(nodes[idx].path.clone()) {
            Ok(entries) => entries,
            Err(_) => {
                nodes[idx].unreadable += 1;
                continue;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                nodes[idx].unreadable += 1;
                continue;
            };
            // Do not follow symlinks: they would double-count and can loop.
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                nodes[idx].unreadable += 1;
                continue;
            };
            if meta.is_dir() {
                nodes[idx].dirs += 1;
                nodes.push(WalkNode {
                    path: entry.path(),
                    parent: Some(idx),
                    depth: nodes[idx].depth + 1,
                    total_bytes: 0,
                    files: 0,
                    dirs: 0,
                    unreadable: 0,
                });
                let child = nodes.len() - 1;
                stack.push(child);
            } else if meta.is_file() {
                nodes[idx].total_bytes += meta.len();
                nodes[idx].files += 1;
            }
        }
    }

    // Children are always created after their parent, so a reverse pass rolls
    // every subtree's counters up one level at a time.
    for idx in (1..nodes.len()).rev() {
        if let Some(parent) = nodes[idx].parent {
            let (head, tail) = nodes.split_at_mut(idx);
            let child = &tail[0];
            let parent_node = &mut head[parent];
            parent_node.total_bytes += child.total_bytes;
            parent_node.files += child.files;
            parent_node.dirs += child.dirs;
            parent_node.unreadable += child.unreadable;
        }
    }

    let limit = max_depth.unwrap_or(usize::MAX);
    let mut rows: Vec<DirUsage> = nodes
        .into_iter()
        .filter(|node| node.depth <= limit)
        .map(|node| DirUsage {
            path: node.path,
            depth: node.depth,
            total_bytes: node.total_bytes,
            files: node.files,
            dirs: node.dirs,
            unreadable: node.unreadable,
        })
        .collect();
    rows.sort_by(|a, b| {
        b.total_bytes
            .cmp(&a.total_bytes)
            .then_with(|| a.path.cmp(&b.path))
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_disk() -> DiskInfo {
        DiskInfo {
            mount_point: "/".into(),
            name: None,
            file_system: Some("apfs".into()),
            read_only: None,
            label: None,
            volume_uuid: None,
            partition_uuid: None,
            device_model: None,
            device_serial: None,
            media_type: None,
            total_bytes: 100,
            available_bytes: 40,
            percent: 60.0,
        }
    }

    #[test]
    fn used_bytes_is_saturating() {
        let disk = sample_disk();
        assert_eq!(disk.used_bytes(), 60);
        assert_eq!(
            DiskInfo {
                available_bytes: 200,
                ..disk
            }
            .used_bytes(),
            0
        );
    }

    #[test]
    fn media_type_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&MediaType::Nvme).unwrap(), "\"nvme\"");
        assert_eq!(MediaType::Removable.to_string(), "removable");
    }

    #[test]
    fn new_disk_fields_are_optional_in_json() {
        let json = serde_json::to_string(&sample_disk()).unwrap();
        assert!(!json.contains("volume_uuid"));
        assert!(!json.contains("media_type"));
    }

    fn make_tree() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("x-disk-walk-{}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("big/sub")).unwrap();
        std::fs::create_dir_all(root.join("small")).unwrap();
        std::fs::write(root.join("root.txt"), vec![7u8; 10]).unwrap();
        std::fs::write(root.join("big/a.bin"), vec![0u8; 100]).unwrap();
        std::fs::write(root.join("big/sub/deep.bin"), vec![0u8; 30]).unwrap();
        std::fs::write(root.join("small/tiny.txt"), vec![0u8; 1]).unwrap();
        root
    }

    #[test]
    fn walk_aggregates_descendants_like_du() {
        let root = make_tree();
        let rows = walk_directory(&root, Some(1));
        assert_eq!(rows.len(), 3); // root + big + small
        assert_eq!(rows[0].path, root);
        assert_eq!(rows[0].total_bytes, 141);
        assert_eq!(rows[0].files, 4);
        assert_eq!(rows[0].dirs, 3); // big, big/sub, small
        assert_eq!(rows[0].unreadable, 0);
        let big = rows.iter().find(|r| r.path.ends_with("big")).unwrap();
        assert_eq!(big.depth, 1);
        assert_eq!(big.total_bytes, 130);
        assert_eq!(big.files, 2);
        assert_eq!(big.dirs, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn walk_without_depth_limit_reports_every_directory() {
        let root = make_tree();
        let rows = walk_directory(&root, None);
        assert_eq!(rows.len(), 4);
        let sub = rows.iter().find(|r| r.path.ends_with("sub")).unwrap();
        assert_eq!(sub.depth, 2);
        assert_eq!(sub.total_bytes, 30);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn walk_counts_unreadable_directories_instead_of_failing() {
        let root = std::env::temp_dir().join(format!("x-disk-walk-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let rows = walk_directory(&root, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].unreadable, 1);
        assert_eq!(rows[0].total_bytes, 0);
    }
}
