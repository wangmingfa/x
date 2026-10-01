//! Linux disk adapter: `sysinfo` for the mount list, then sysfs and the udev
//! symlinks under `/dev/disk` for the physical facts.
//!
//! Layer 1/2 only — no command is spawned:
//!
//! * `/dev/disk/by-uuid`, `by-label` and `by-partuuid` are maintained by udev
//!   and map stable identifiers to `/dev/...` nodes.
//! * `/sys/block/<disk>/queue/rotational`, `device/model`, `device/serial` and
//!   `removable` describe the whole disk behind a partition; stacked devices
//!   (`dm-*`) are unwrapped through their `slaves` list.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use x_core::disk::{DiskInfo, DiskManager, MediaType};
use x_core::error::Result;

use crate::common::disk_sysinfo::SysinfoDisk;

/// Lists mounts and decorates them with the physical-disk facts.
#[derive(Debug, Default)]
pub struct LinuxDisk;

impl LinuxDisk {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }
}

impl DiskManager for LinuxDisk {
    fn list(&self) -> Result<Vec<DiskInfo>> {
        let mut rows = SysinfoDisk::new().list()?;
        let by_uuid = symlink_map("/dev/disk/by-uuid");
        let by_label = symlink_map("/dev/disk/by-label");
        let by_partuuid = symlink_map("/dev/disk/by-partuuid");

        for row in &mut rows {
            let device = row.name.clone().unwrap_or_default();
            let on_disk = device.starts_with("/dev/");
            if on_disk {
                row.volume_uuid = lookup(&by_uuid, &device);
                row.label = lookup(&by_label, &device);
                row.partition_uuid = lookup(&by_partuuid, &device);
            }
            let fs = row.file_system.as_deref().unwrap_or("");
            if !on_disk && fs == "overlay" {
                row.media_type = Some(MediaType::Virtual);
                continue;
            }
            let Some(disk) = whole_disk_for(Path::new(&device), 0) else {
                continue;
            };
            row.device_model = read_sysfs_string(&format!("/sys/block/{disk}/device/model"));
            row.device_serial = read_sysfs_string(&format!("/sys/block/{disk}/device/serial"));
            let removable = sysfs_flag(&format!("/sys/block/{disk}/removable"));
            let rotational = sysfs_flag(&format!("/sys/block/{disk}/queue/rotational"));
            row.media_type = classify_media(&disk, removable.unwrap_or(false), rotational, fs);
        }
        Ok(rows)
    }
}

/// `(identifier -> /dev node)` for one `/dev/disk/by-*` directory.
fn symlink_map(dir: &str) -> BTreeMap<String, PathBuf> {
    let mut map = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return map;
    };
    for entry in entries.flatten() {
        if let Ok(target) = std::fs::canonicalize(entry.path()) {
            map.insert(entry.file_name().to_string_lossy().into_owned(), target);
        }
    }
    map
}

fn lookup(map: &BTreeMap<String, PathBuf>, device: &str) -> Option<String> {
    let target = std::fs::canonicalize(device).ok()?;
    map.iter()
        .find(|(_, resolved)| *resolved == &target)
        .map(|(key, _)| key.clone())
}

/// Read a sysfs file, trimming the padding some attributes carry (model and
/// serial are space-padded to the field width).
fn read_sysfs_string(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn sysfs_flag(path: &str) -> Option<bool> {
    read_sysfs_string(path).map(|value| value == "1")
}

/// Whole-disk sysfs name for a device node, following `dm-*` slaves.
fn whole_disk_for(device: &Path, depth: u8) -> Option<String> {
    if depth > 3 {
        return None;
    }
    let resolved = std::fs::canonicalize(device).ok()?; // /dev/mapper/x -> /dev/dm-0
    let base = resolved.file_name()?.to_str()?.to_string();
    let block_dir = Path::new("/sys/block");
    if block_dir.join(&base).exists() {
        // Stacked devices (`dm-*`, `md*`) sit in /sys/block themselves but
        // borrow their capacity from real disks; `slaves` names those.
        if let Some(slave) = slave_names(&base).into_iter().next() {
            return whole_disk_for(Path::new("/dev").join(&slave).as_path(), depth + 1);
        }
        return Some(base);
    }
    if let Some(stripped) = strip_partition_suffix(&base) {
        if block_dir.join(&stripped).exists() {
            return Some(stripped);
        }
    }
    // Partition directories hang under their disk in sysfs.
    let parent = std::fs::canonicalize(block_dir.join(&base).join("..")).ok()?;
    let name = parent.file_name()?.to_str()?.to_owned();
    block_dir.join(&name).exists().then_some(name)
}

/// Names listed in `/sys/block/<dm>/slaves`, empty for non-stacked devices.
fn slave_names(base: &str) -> Vec<String> {
    let raw = match std::fs::read_to_string(format!("/sys/block/{base}/slaves")) {
        Ok(raw) => raw,
        Err(_) => return Vec::new(),
    };
    raw.split_whitespace()
        .map(|name| name.to_string())
        .collect()
}

/// Strip a partition suffix from a block device name.
///
/// `sda2 -> sda`, `nvme0n1p2 -> nvme0n1`, `mmcblk0p1 -> mmcblk0`. Whole-disk
/// names (`sda`, `nvme0n1`, `dm-0`) return `None`.
fn strip_partition_suffix(name: &str) -> Option<String> {
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        let cut = name.rfind('p')?;
        let digits = &name[cut + 1..];
        return (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            .then(|| name[..cut].to_string());
    }
    // SCSI/virtio/xen style: trailing decimal index on an alphabetic stem.
    let digits_len = name.chars().rev().take_while(char::is_ascii_digit).count();
    if digits_len == 0 || digits_len == name.len() {
        return None;
    }
    let (stem, digits) = name.split_at(name.len() - digits_len);
    let plausible = digits.bytes().all(|b| b.is_ascii_digit())
        && stem.ends_with(|c: char| c.is_ascii_alphabetic())
        && (name.starts_with("sd") || name.starts_with("vd") || name.starts_with("xd"));
    plausible.then(|| stem.to_string())
}

/// Storage class of the whole disk, from its name and sysfs flags.
fn classify_media(
    disk: &str,
    removable: bool,
    rotational: Option<bool>,
    file_system: &str,
) -> Option<MediaType> {
    if matches!(
        file_system,
        "nfs"
            | "nfs4"
            | "cifs"
            | "smb"
            | "smb2"
            | "smb3"
            | "fuse.sshfs"
            | "sshfs"
            | "9p"
            | "lustre"
    ) {
        return Some(MediaType::Network);
    }
    if ["loop", "dm-", "zram", "md", "ram", "bfsmount"]
        .iter()
        .any(|prefix| disk.starts_with(prefix))
    {
        return Some(MediaType::Virtual);
    }
    if removable {
        return Some(MediaType::Removable);
    }
    if disk.starts_with("nvme") {
        return Some(MediaType::Nvme);
    }
    if disk.starts_with("mmcblk") {
        return Some(MediaType::Ssd);
    }
    match rotational {
        Some(true) => Some(MediaType::Hdd),
        Some(false) => Some(MediaType::Ssd),
        None => None,
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn DiskManager> {
    std::sync::Arc::new(LinuxDisk::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_suffixes_strip_only_for_partitions() {
        for (input, expected) in [
            ("sda2", Some("sda")),
            ("vda3", Some("vda")),
            ("xvda1", Some("xvda")),
            ("nvme0n1p2", Some("nvme0n1")),
            ("mmcblk0p1", Some("mmcblk0")),
            ("sda", None),
            ("nvme0n1", None),
            ("dm-0", None),
            ("loop0", None),
            ("mmcblk0", None),
        ] {
            assert_eq!(
                strip_partition_suffix(input).as_deref(),
                expected,
                "{input}"
            );
        }
    }

    #[test]
    fn media_classifies_by_name_then_flags() {
        assert_eq!(
            classify_media("sda", false, Some(true), "ext4"),
            Some(MediaType::Hdd)
        );
        assert_eq!(
            classify_media("sda", false, Some(false), "ext4"),
            Some(MediaType::Ssd)
        );
        assert_eq!(
            classify_media("nvme0n1", false, Some(false), "ext4"),
            Some(MediaType::Nvme)
        );
        assert_eq!(
            classify_media("sdb", true, Some(true), "vfat"),
            Some(MediaType::Removable)
        );
        assert_eq!(
            classify_media("sda", false, Some(false), "nfs4"),
            Some(MediaType::Network)
        );
        assert_eq!(
            classify_media("dm-0", false, Some(false), "ext4"),
            Some(MediaType::Virtual)
        );
        // A disk with no rotational attribute stays unknown.
        assert_eq!(classify_media("sda", false, None, "ext4"), None);
    }

    #[test]
    fn mounts_are_decorated_where_the_kernel_knows() {
        let rows = LinuxDisk::new().list().expect("list");
        assert!(!rows.is_empty());
        // The container this may run inside has no /dev/disk symlinks, so only
        // the invariant is asserted: fields stay coherent.
        for row in &rows {
            assert!(
                !(row.volume_uuid.is_some()
                    && row.name.as_deref().unwrap_or_default() == "overlay"),
                "pseudo filesystems cannot carry a UUID: {row:?}"
            );
        }
    }
}
