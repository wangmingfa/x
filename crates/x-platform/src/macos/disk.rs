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

use core_foundation_sys::base::CFRelease;
use core_foundation_sys::dictionary::{
    CFDictionaryGetValue, CFDictionaryRef, CFMutableDictionaryRef,
};
use core_foundation_sys::number::{kCFNumberSInt64Type, CFNumberGetValue, CFNumberRef};
use core_foundation_sys::string::{CFStringCreateWithCString, CFStringRef};
use x_core::disk::{DiskInfo, DiskIo, DiskManager, MediaType};
use x_core::error::{Error, Result};

use crate::common::disk_sysinfo::SysinfoDisk;

// Minimal IOKit registry access for block-device statistics. The C API is
// plain enough that binding it here beats pulling another crate.
type IoObject = u32;
type KernReturn = i32;
const KERN_SUCCESS: KernReturn = 0;
const MAIN_PORT_DEFAULT: IoObject = 0;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn IOServiceGetMatchingServices(
        main_port: IoObject,
        matching: *mut std::ffi::c_void,
        existing: *mut IoObject,
    ) -> KernReturn;
    fn IOIteratorNext(iterator: IoObject) -> IoObject;
    fn IOObjectRelease(object: IoObject) -> KernReturn;
    fn IORegistryEntryCreateCFProperties(
        entry: IoObject,
        properties: *mut CFMutableDictionaryRef,
        allocator: *mut std::ffi::c_void,
        options: u32,
    ) -> KernReturn;
    fn IORegistryEntryCreateIterator(
        entry: IoObject,
        plane: *const std::ffi::c_char,
        options: u32,
        iterator: *mut IoObject,
    ) -> KernReturn;
}

extern "C" {
    fn CFDictionaryGetCount(dictionary: CFDictionaryRef) -> std::os::raw::c_ulong;
    fn CFStringGetCString(
        the_string: *const std::ffi::c_void,
        buffer: *mut std::ffi::c_char,
        buffer_size: std::os::raw::c_long,
        encoding: u32,
    ) -> u8;
}

const K_CFSTRING_ENCODING_UTF8: u32 = 0x0800_0100;
const IO_REGISTRY_PLANE: &[u8] = b"IOService\0";
/// `kIORegistryIterateRecursively`: descend into each returned entry.
const K_IO_REGISTRY_ITERATE_RECURSIVELY: u32 = 0x0000_0001;
/// Read the "Statistics" dictionary of one `IOBlockStorageDriver` entry and
/// pick the four counters `x disk io` reports. Keys are confirmed by ioreg
/// output, including the spaces and parentheses — any change there reads as
/// absence, never as a zero.
unsafe fn statistics_of(entry: IoObject) -> Option<(u64, u64, u64, u64)> {
    let mut properties: CFMutableDictionaryRef = std::ptr::null_mut();
    // SAFETY: `properties` is our own storage, filled in with a +1 reference
    // we release below; the allocator argument is the default (null).
    if unsafe { IORegistryEntryCreateCFProperties(entry, &mut properties, std::ptr::null_mut(), 0) }
        != KERN_SUCCESS
        || properties.is_null()
    {
        return None;
    }
    let stats: Option<(u64, u64, u64, u64)> = unsafe {
        let key = |name: &str| cf_string(name);
        let read_key = key("Statistics");
        let stats_ref =
            CFDictionaryGetValue(properties, read_key as *const _) as CFMutableDictionaryRef;
        CFRelease(read_key as *const _);
        if stats_ref.is_null() || CFDictionaryGetCount(stats_ref) == 0 {
            CFRelease(properties as *const _);
            return None;
        }
        let get_u64 = |dict: CFDictionaryRef, name: &str| -> Option<u64> {
            let cfkey = cf_string(name);
            let value = CFDictionaryGetValue(dict, cfkey as *const _);
            CFRelease(cfkey as *const _);
            if value.is_null() {
                return None;
            }
            let mut out: u64 = 0;
            // SAFETY: `value` is a CFNumber from the dictionary and `out`
            // outlives the call; the cast matches its expected C type.
            CFNumberGetValue(
                value as CFNumberRef,
                kCFNumberSInt64Type,
                &mut out as *mut u64 as *mut std::ffi::c_void,
            )
            .then_some(out)
        };
        let result = Some((
            get_u64(stats_ref, "Bytes (Read)")?,
            get_u64(stats_ref, "Bytes (Write)")?,
            get_u64(stats_ref, "Operations (Read)")?,
            get_u64(stats_ref, "Operations (Write)")?,
        ));
        CFRelease(properties as *const _);
        result
    };
    stats
}

/// Create a `CFString` from a Rust string; caller releases (+1 reference).
fn cf_string(text: &str) -> CFStringRef {
    let c = std::ffi::CString::new(text).unwrap_or_default();
    // SAFETY: `c` is NUL terminated and lives for the call.
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), K_CFSTRING_ENCODING_UTF8) }
}

/// BSD name (`disk0`) of the whole disk an `IOBlockStorageDriver` serves.
///
/// The registry hangs `IOMedia` (and its `IOMediaBSDClient`, which carries
/// `BSD Name`) *below* the driver, so the driver's own subtree is iterated
/// recursively rather than walking parents or siblings. `None` when the
/// subtree has no `IOMedia` at all — an empty card slot.
unsafe fn bsd_name_of(entry: IoObject) -> Option<String> {
    unsafe {
        let mut iterator: IoObject = 0;
        // SAFETY: `entry` is a live registry object the caller owns; the
        // iterator is released below.
        if IORegistryEntryCreateIterator(
            entry,
            IO_REGISTRY_PLANE.as_ptr() as *const _,
            K_IO_REGISTRY_ITERATE_RECURSIVELY,
            &mut iterator,
        ) != KERN_SUCCESS
            || iterator == 0
        {
            return None;
        }
        let mut result = None;
        loop {
            let child = IOIteratorNext(iterator);
            if child == 0 {
                break;
            }
            let candidate = registry_string(child, "BSD Name");
            IOObjectRelease(child);
            if candidate.is_some() {
                result = candidate;
                break;
            }
        }
        IOObjectRelease(iterator);
        result
    }
}

/// Read one string property off a registry entry.
unsafe fn registry_string(entry: IoObject, key_name: &str) -> Option<String> {
    let mut properties: CFMutableDictionaryRef = std::ptr::null_mut();
    // SAFETY: `properties` is our own storage, filled in with a +1 reference
    // we release below; the null allocator is the default.
    let found = unsafe {
        IORegistryEntryCreateCFProperties(entry, &mut properties, std::ptr::null_mut(), 0)
    } == KERN_SUCCESS
        && !properties.is_null();
    if !found {
        return None;
    }
    let key = cf_string(key_name);
    // SAFETY: `key` and `properties` are valid CF objects for the calls below.
    let value = unsafe { CFDictionaryGetValue(properties, key as *const _) };
    unsafe { CFRelease(key as *const _) };
    let out = if value.is_null() {
        None
    } else {
        cf_string_value(value)
    };
    unsafe { CFRelease(properties as *const _) };
    out
}

/// Copy a `CFString` reference into an owned Rust string (does not release).
unsafe fn cf_string_value(value: *const std::ffi::c_void) -> Option<String> {
    let mut buffer = [0u8; 64];
    // SAFETY: `value` is a valid CFString, `buffer` outlives the call.
    let ok = unsafe {
        CFStringGetCString(
            value,
            buffer.as_mut_ptr() as *mut std::ffi::c_char,
            buffer.len() as std::os::raw::c_long,
            K_CFSTRING_ENCODING_UTF8,
        )
    };
    if ok == 0 {
        return None;
    }
    Some(
        std::ffi::CStr::from_bytes_until_nul(&buffer)
            .ok()?
            .to_string_lossy()
            .into_owned(),
    )
}

/// Cumulative I/O counters per whole disk, from the `IOBlockStorageDriver`
/// statistics dictionaries in the IOKit registry.
fn io_via_iokit() -> Result<Vec<DiskIo>> {
    let name =
        std::ffi::CString::new("IOBlockStorageDriver").map_err(|e| Error::system(e.to_string()))?;
    // SAFETY: each extern call below uses valid handles or our own storage as
    // documented by IOKit; every registry object we receive is released.
    unsafe {
        let matching = service_matching(&name);
        if matching.is_null() {
            return Err(Error::system("IOServiceMatching returned null"));
        }
        let mut iterator: IoObject = 0;
        if IOServiceGetMatchingServices(MAIN_PORT_DEFAULT, matching, &mut iterator) != KERN_SUCCESS
        {
            return Err(Error::system("no IOBlockStorageDriver services"));
        }
        let mut rows = Vec::new();
        loop {
            let entry = IOIteratorNext(iterator);
            if entry == 0 {
                break;
            }
            // A driver whose subtree carries no `IOMedia` serves no disk: an
            // empty SD card slot reports statistics for no `diskN`. Without a
            // BSD name there is nothing to diff against next round either, so
            // the entry is skipped rather than shown under a made-up name.
            if let (Some((read_bytes, write_bytes, read_ops, write_ops)), Some(device)) =
                (statistics_of(entry), bsd_name_of(entry))
            {
                rows.push(DiskIo {
                    device,
                    read_bytes,
                    write_bytes,
                    read_ops,
                    write_ops,
                });
            }
            IOObjectRelease(entry);
        }
        IOObjectRelease(iterator);
        rows.sort_by(|a, b| a.device.cmp(&b.device));
        Ok(rows)
    }
}

/// `IOServiceMatching` for one service class; the returned dictionary is
/// consumed by `IOServiceGetMatchingServices`.
unsafe fn service_matching(name: &std::ffi::CStr) -> *mut std::ffi::c_void {
    // SAFETY: `name` is NUL terminated; the returned dictionary is consumed
    // by `IOServiceGetMatchingServices`.
    unsafe { IOServiceMatching(name.as_ptr()) }
}

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

    fn io(&self) -> Result<Vec<DiskIo>> {
        io_via_iokit()
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
            return rest.split_whitespace().next();
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
