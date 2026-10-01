//! macOS disk adapter: `sysinfo` for the mount list, then `diskutil info
//! -plist` for the volume label, UUIDs and the medium classification.
//!
//! This is a deliberate level-3 source: the native alternatives are the
//! deprecated DiskArbitration framework and raw IOKit registry queries, and
//! neither is bound by the workspace's dependencies. The `-plist` form is
//! machine-readable XML with fixed English keys, so — unlike the text form of
//! `diskutil` — it survives localized systems, which is why the policy against
//! parsing command output does not forbid it here.

use std::process::Command;

use x_core::disk::{DiskInfo, DiskManager, MediaType};
use x_core::error::Result;

use crate::common::disk_sysinfo::SysinfoDisk;

/// Lists mounts and decorates them with what `diskutil` knows.
#[derive(Debug, Default)]
pub struct MacDisk;

impl MacDisk {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }
}

impl DiskManager for MacDisk {
    fn list(&self) -> Result<Vec<DiskInfo>> {
        let mut rows = SysinfoDisk::new().list()?;
        for row in &mut rows {
            let target = volume_target(row);
            let Some(plist) = diskutil_info(&target) else {
                continue;
            };
            row.label = plist_string(&plist, "VolumeName");
            row.volume_uuid = plist_string(&plist, "VolumeUUID");
            row.partition_uuid = plist_string(&plist, "PartitionUUID");
            let fs = row.file_system.as_deref().unwrap_or("");
            let protocol = first_string(&plist, &["Device Protocol", "Protocol", "BusContent"]);
            row.media_type = mac_media_type(
                fs,
                protocol.as_deref(),
                plist_bool(&plist, "Internal"),
                plist_bool(&plist, "Ejectable"),
                plist_bool(&plist, "SolidStateMedia"),
            );
            if row.device_model.is_none() {
                // Volumes on the same physical disk share one parent entry; the
                // query is cheap, so no caching layer is worth it for a handful
                // of mounts.
                if let Some(parent) = plist_string(&plist, "ParentWholeDisk") {
                    if let Some(parent_plist) = diskutil_info(&parent) {
                        row.device_model = first_string(
                            &parent_plist,
                            &["MediaName", "IOMediaContent", "DeviceModel"],
                        );
                        let protocol = first_string(
                            &parent_plist,
                            &["Device Protocol", "Protocol", "BusContent"],
                        );
                        row.media_type = mac_media_type(
                            fs,
                            protocol.as_deref(),
                            plist_bool(&parent_plist, "Internal"),
                            plist_bool(&parent_plist, "Ejectable"),
                            plist_bool(&parent_plist, "SolidStateMedia"),
                        );
                    }
                }
            }
        }
        Ok(rows)
    }
}

/// What to hand `diskutil info`: the mount point, or the recorded device node
/// if sysinfo showed one.
fn volume_target(row: &DiskInfo) -> String {
    match row.name.as_deref() {
        Some(node) if node.starts_with("/dev/") => node.to_string(),
        _ => row.mount_point.clone(),
    }
}

fn diskutil_info(target: &str) -> Option<String> {
    let output = Command::new("diskutil")
        .args(["info", "-plist", target])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// First key that yields a string value.
fn first_string(plist: &str, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| plist_string(plist, key))
}

/// The raw value token that follows `<key>name</key>`, trimmed.
fn plist_value<'a>(plist: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("<key>{key}</key>");
    let rest = plist.find(&marker)? + marker.len();
    let rest = plist[rest..].trim_start();
    let open = match rest {
        s if s.starts_with("<string>") => "<string>",
        s if s.starts_with("<true/>") || s.starts_with("<false/>") => {
            return Some(rest.split_whitespace().next()?);
        }
        _ => return None,
    };
    let body = rest[open.len()..].find("</")?;
    Some(&rest[open.len()..open.len() + body])
}

fn plist_string(plist: &str, key: &str) -> Option<String> {
    let value = plist_value(plist, key)?;
    let unescaped = unescape_xml(value);
    (!unescaped.is_empty()).then_some(unescaped)
}

fn plist_bool(plist: &str, key: &str) -> Option<bool> {
    match plist_value(plist, key)? {
        "<true/>" => Some(true),
        "<false/>" => Some(false),
        _ => None,
    }
}

fn unescape_xml(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Classify the medium from the filesystem type and the volume/disk flags.
fn mac_media_type(
    file_system: &str,
    protocol: Option<&str>,
    internal: Option<bool>,
    ejectable: Option<bool>,
    solid_state: Option<bool>,
) -> Option<MediaType> {
    let fs = file_system.to_ascii_lowercase();
    if ["smb", "afp", "nfs", "cifs", "webdav", "davfs"]
        .iter()
        .any(|needle| fs.contains(needle))
    {
        return Some(MediaType::Network);
    }
    if internal == Some(false)
        && (ejectable == Some(true) || protocol.is_some_and(|p| p.eq_ignore_ascii_case("USB")))
    {
        return Some(MediaType::Removable);
    }
    if protocol.is_some_and(|p| {
        let p = p.to_ascii_lowercase();
        p.contains("nvme") || p.contains("pci")
    }) {
        return Some(MediaType::Nvme);
    }
    match solid_state {
        Some(true) => Some(MediaType::Ssd),
        Some(false) => Some(MediaType::Hdd),
        None => None,
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn DiskManager> {
    std::sync::Arc::new(MacDisk::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>VolumeName</key>
	<string>Macintosh &amp; Data</string>
	<key>VolumeUUID</key>
	<string>1F2E3D4C-5B6A-7890-ABCD-EF1234567890</string>
	<key>Ejectable</key>
	<false/>
	<key>Internal</key>
	<true/>
	<key>SolidStateMedia</key>
	<true/>
	<key>MediaName</key>
	<string>AppleAPFSMedia</string>
</dict>
</plist>"#;

    #[test]
    fn plist_strings_and_bools_are_extracted() {
        assert_eq!(
            plist_string(SAMPLE, "VolumeName").as_deref(),
            Some("Macintosh & Data")
        );
        assert_eq!(
            plist_string(SAMPLE, "VolumeUUID").as_deref(),
            Some("1F2E3D4C-5B6A-7890-ABCD-EF1234567890")
        );
        assert_eq!(plist_bool(SAMPLE, "Internal"), Some(true));
        assert_eq!(plist_bool(SAMPLE, "Ejectable"), Some(false));
        assert_eq!(plist_bool(SAMPLE, "SolidStateMedia"), Some(true));
        assert_eq!(plist_string(SAMPLE, "NoSuchKey"), None);
        assert_eq!(plist_bool(SAMPLE, "MediaName"), None);
    }

    #[test]
    fn media_type_prefers_the_strongest_signal() {
        assert_eq!(
            mac_media_type("smbfs", None, None, None, None),
            Some(MediaType::Network)
        );
        assert_eq!(
            mac_media_type("apfs", Some("NVMe"), Some(true), Some(false), Some(true)),
            Some(MediaType::Nvme)
        );
        assert_eq!(
            mac_media_type("apfs", None, Some(true), Some(false), Some(true)),
            Some(MediaType::Ssd)
        );
        assert_eq!(
            mac_media_type("hfsplus", Some("USB"), Some(false), Some(true), None),
            Some(MediaType::Removable)
        );
        assert_eq!(mac_media_type("apfs", None, None, None, None), None);
    }

    #[test]
    fn the_target_prefers_an_explicit_device_node() {
        let row = DiskInfo {
            mount_point: "/Volumes/USB".into(),
            name: Some("/dev/disk4s2".into()),
            file_system: None,
            read_only: None,
            label: None,
            volume_uuid: None,
            partition_uuid: None,
            device_model: None,
            device_serial: None,
            media_type: None,
            total_bytes: 1,
            available_bytes: 1,
            percent: 0.0,
        };
        assert_eq!(volume_target(&row), "/dev/disk4s2");
        let row = DiskInfo {
            name: Some("disk4s2".into()),
            ..row
        };
        assert_eq!(volume_target(&row), "/Volumes/USB");
    }
}
