//! Cross-platform hardware device inventory.
//!
//! Each OS keeps its own enumeration vocabulary, so this adapter shells out
//! to the platform's own tool and parses its structured output rather than
//! trying to be clever with libc ioctls.
//!
//! - **Windows** — `Get-PnpDevice -PresentOnly` emitted as one JSON document.
//!   The full class vocabulary (`Display`, `HIDClass`, `WPD`, …) survives via
//!   `class_raw`; the normalized `class` buckets only the families users ask
//!   for with subcommands.
//! - **Linux** — sysfs: `/sys/bus/usb/devices`, `/sys/class/{bluetooth,drm,
//!   input,net,video4linux,sound}` and `/proc/asound/cards`. Every fact is a
//!   `read_trim` of a kernel attribute; the absence of a file means the field
//!   is absent, never fabricated.
//! - **macOS** — `system_profiler` per data type, each a JSON document.
//!   The USB tree is a hierarchy; it is walked with a depth cap and every
//!   node at or below the cap becomes one row.
//!
//! Everything is a read: no confirmation, no elevation, no audit.

#[cfg(any(windows, target_os = "macos"))]
use std::process::Command;

use x_core::device::{DeviceClass, DeviceInfo, DeviceManager};
#[cfg(any(windows, target_os = "macos"))]
use x_core::error::Error;
use x_core::error::Result;

/// Adapter installed into the composition root.
#[derive(Debug, Default)]
pub struct PlatformDevices;

/// Result of a device read that may simply be "not present on this OS".
impl DeviceManager for PlatformDevices {
    fn devices(&self) -> Result<Vec<DeviceInfo>> {
        #[cfg(windows)]
        {
            enumerate_pnp()
        }
        #[cfg(target_os = "linux")]
        {
            enumerate_sysfs()
        }
        #[cfg(target_os = "macos")]
        {
            enumerate_profiler()
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            Err(Error::unsupported(
                "no hardware enumerator is compiled in for this platform",
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Windows: Get-PnpDevice
// ---------------------------------------------------------------------------

/// Ask Windows for every *present* PnP device as compact JSON.
///
/// `Get-PnpDevice` already exists in every modern Windows shell. The
/// `Select-Object` keeps the projection stable against future CIM property
/// additions; `ConvertTo-Json` turns it into one line per whole result set
/// which `parse_pnp_json` then normalises to an array.
#[cfg(windows)]
fn enumerate_pnp() -> Result<Vec<DeviceInfo>> {
    let script = "$ErrorActionPreference='Continue';\
        Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue \
        | Select-Object Class,FriendlyName,Manufacturer,Status,InstanceId \
        | ConvertTo-Json -Compress -Depth 2";
    let text = run_powershell_json(script)?;
    parse_pnp_json(&text)
}

/// Turn the `ConvertTo-Json` shape (empty string / one object / array) into
/// an array of `DeviceInfo`. Empty is a legitimate success (a machine with
/// devices the caller can see none of).
#[cfg(windows)]
pub fn parse_pnp_json(text: &str) -> Result<Vec<DeviceInfo>> {
    let text = text.trim();
    if text.is_empty() || text == "null" {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| Error::system(format!("cannot parse Get-PnpDevice JSON: {e}")))?;
    let rows = match value {
        serde_json::Value::Array(rows) => rows,
        other => vec![other],
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        // A PnP entity is only useful if it has a name to show.
        let name = json_str(&row, "FriendlyName").unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let class_raw = json_str(&row, "Class");
        let class = class_raw
            .as_deref()
            .map(DeviceClass::parse)
            .unwrap_or(DeviceClass::Other);
        out.push(DeviceInfo {
            name,
            class,
            id: json_str(&row, "InstanceId"),
            class_raw,
            status: json_str(&row, "Status"),
            manufacturer: json_str(&row, "Manufacturer"),
        });
    }
    Ok(out)
}

#[cfg(windows)]
fn json_str(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(windows)]
fn run_powershell_json(script: &str) -> Result<String> {
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .map_err(|e| Error::system(format!("cannot run powershell.exe: {e}")))?;
    // Filter device queries are read-only; PS5 writes nothing to stderr on a
    // quiet success, so the whole stdout is the JSON document.
    if !output.status.success() {
        let stderr = crate::windows::service::decode_console(&output.stderr);
        let stderr = stderr.trim();
        return Err(Error::system(if stderr.is_empty() {
            format!("powershell.exe failed (exit {:?})", output.status.code())
        } else {
            format!("powershell.exe failed: {stderr}")
        }));
    }
    Ok(crate::windows::service::decode_console(&output.stdout))
}

// ---------------------------------------------------------------------------
// Linux: sysfs
// ---------------------------------------------------------------------------

/// Attribute files return trailing newlines; empty or missing is `None`.
#[cfg(target_os = "linux")]
fn read_attr(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Enumerate the sysfs classes we understand, plus the ALSA card list.
#[cfg(target_os = "linux")]
fn enumerate_sysfs() -> Result<Vec<DeviceInfo>> {
    let mut out = Vec::new();
    out.extend(usb_devices());
    out.extend(class_devices(
        "/sys/class/bluetooth",
        DeviceClass::Bluetooth,
    ));
    out.extend(class_devices("/sys/class/net", DeviceClass::Network));
    out.extend(class_devices("/sys/class/input", DeviceClass::Input));
    out.extend(display_devices());
    out.extend(video_devices());
    out.extend(alsa_cards());
    Ok(out)
}

/// USB: `product` is the human name; `idVendor`/`idProduct` identify it.
#[cfg(target_os = "linux")]
fn usb_devices() -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    for dir in sysfs_entries("/sys/bus/usb/devices") {
        let name = read_attr(&dir.join("product")).unwrap_or_else(|| {
            dir.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        if name.is_empty() {
            continue;
        }
        out.push(DeviceInfo {
            name,
            class: DeviceClass::Usb,
            class_raw: Some("usb".to_string()),
            status: None,
            manufacturer: read_attr(&dir.join("manufacturer")),
            id: usb_id(&dir),
        });
    }
    out
}

/// Build `idVendor:idProduct` when both are present.
#[cfg(target_os = "linux")]
fn usb_id(dir: &std::path::Path) -> Option<String> {
    let vendor = read_attr(&dir.join("idVendor"))?;
    let product = read_attr(&dir.join("idProduct"))?;
    Some(format!("{vendor}:{product}"))
}

/// Generic `/sys/class/<name>` walk: the directory entry is the identifier.
#[cfg(target_os = "linux")]
fn class_devices(root: &str, class: DeviceClass) -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    for dir in sysfs_entries(root) {
        let name = dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        out.push(DeviceInfo {
            name: name.clone(),
            class,
            class_raw: Some(class.label().to_string()),
            status: read_attr(&dir.join("state")).or_else(|| read_attr(&dir.join("status"))),
            manufacturer: read_attr(&dir.join("manufacturer")),
            id: Some(name),
        });
    }
    out
}

/// DRM connectors: `status` says connected/disconnected, the connector name
/// (e.g. `HDMI-A-1`) is the row label.
#[cfg(target_os = "linux")]
fn display_devices() -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    for dir in sysfs_entries("/sys/class/drm") {
        let name = dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.contains('-') {
            // `card0`, `renderD128` etc. carry no connector identity.
            continue;
        }
        out.push(DeviceInfo {
            name: name.clone(),
            class: DeviceClass::Display,
            class_raw: Some("drm".to_string()),
            status: read_attr(&dir.join("status")),
            manufacturer: None,
            id: Some(name),
        });
    }
    out
}

/// Video4Linux capture nodes are the camera surfaces of sysfs.
#[cfg(target_os = "linux")]
fn video_devices() -> Vec<DeviceInfo> {
    class_devices_named("/sys/class/video4linux", DeviceClass::Camera, "video")
}

/// Entries whose name starts with a fixed prefix (only capture nodes).
#[cfg(target_os = "linux")]
fn class_devices_named(root: &str, class: DeviceClass, prefix: &str) -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    for entry in sysfs_entries(root) {
        let name = entry
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.starts_with(prefix) {
            continue;
        }
        out.push(DeviceInfo {
            name: name.clone(),
            class,
            class_raw: Some(prefix.to_string()),
            status: None,
            manufacturer: None,
            id: Some(name),
        });
    }
    out
}

/// Parse `/proc/asound/cards`, which is fixed-column human text, not a tree.
///
/// ```text
///  0 [PCH            ]: HDA-Intel - HDA Intel PCH
///                       HDA Intel PCH at 0xf7e14000 irq 123
/// ```
/// Card lines carry `]: `; the indented detail lines below them do not, and
/// neither does the `--- no soundcards ---` placeholder.
#[cfg(target_os = "linux")]
pub fn alsa_cards_text_to_devices(text: &str) -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.split_once("]: ").map(|(_, after)| after) else {
            continue;
        };
        // `<driver> - <description>`
        let Some((driver, description)) = rest.split_once(" - ") else {
            continue;
        };
        out.push(DeviceInfo {
            name: description.trim().to_string(),
            class: DeviceClass::Audio,
            class_raw: Some("alsa".to_string()),
            status: None,
            manufacturer: None,
            id: Some(driver.trim().to_string()),
        });
    }
    out
}

#[cfg(target_os = "linux")]
fn alsa_cards() -> Vec<DeviceInfo> {
    let text = std::fs::read_to_string("/proc/asound/cards").unwrap_or_default();
    alsa_cards_text_to_devices(&text)
}

/// List the subdirectories of a sysfs class root as `PathBuf`s.
#[cfg(target_os = "linux")]
fn sysfs_entries(root: &str) -> Vec<std::path::PathBuf> {
    let Ok(read_dir) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    read_dir
        .filter_map(|entry| entry.ok())
        .map(|e| e.path())
        .collect()
}

// ---------------------------------------------------------------------------
// macOS: system_profiler
// ---------------------------------------------------------------------------

/// How deep to walk the USB hierarchy before stopping. Real machines nest a
/// hub chain two or three levels deep; `usize::MAX` would flatten even the
/// per-port endpoints which are noise.
#[cfg(target_os = "macos")]
const USB_DEPTH_CAP: usize = 3;

/// One `system_profiler <SPType> -json` document → JSON value.
#[cfg(target_os = "macos")]
fn profiler_json(data_type: &str) -> Result<serde_json::Value> {
    let output = Command::new("system_profiler")
        .args([data_type, "-json"])
        .output()
        .map_err(|e| Error::system(format!("cannot run system_profiler: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(Error::system(if stderr.is_empty() {
            format!(
                "system_profiler {data_type} failed (exit {:?})",
                output.status.code()
            )
        } else {
            format!("system_profiler {data_type} failed: {stderr}")
        }));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .map_err(|e| Error::system(format!("cannot parse system_profiler {data_type}: {e}")))
}

/// Enumerate the device classes `system_profiler` can be asked about.
#[cfg(target_os = "macos")]
fn enumerate_profiler() -> Result<Vec<DeviceInfo>> {
    let mut out = Vec::new();
    // Each data type is its own document: a type missing on this machine
    // (no camera, no bluetooth) yields an empty root object, never an error.
    let audio = profiler_json("SPAudioDataType").unwrap_or_else(|_| serde_json::json!({}));
    let displays = profiler_json("SPDisplaysDataType").unwrap_or_else(|_| serde_json::json!({}));
    let usb = profiler_json("SPUSBDataType").unwrap_or_else(|_| serde_json::json!({}));
    let camera = profiler_json("SPCameraDataType").unwrap_or_else(|_| serde_json::json!({}));
    let bluetooth = profiler_json("SPBluetoothDataType").unwrap_or_else(|_| serde_json::json!({}));
    collect_flat(&audio, "SPAudioDataType", DeviceClass::Audio, &mut out);
    collect_flat(
        &displays,
        "SPDisplaysDataType",
        DeviceClass::Display,
        &mut out,
    );
    collect_flat(&camera, "SPCameraDataType", DeviceClass::Camera, &mut out);
    collect_flat(
        &bluetooth,
        "SPBluetoothDataType",
        DeviceClass::Bluetooth,
        &mut out,
    );
    collect_usb(&usb, "SPUSBDataType", 0, &mut out);
    Ok(out)
}

/// Profiler JSON is always `{"<DataType>": {"items": [ ... ]}}` (older) or
/// `{"<DataType>": { ...tree... }}` (newer). Pull the named items one level
/// below the root, keeping any object that has a displayable name.
#[cfg(target_os = "macos")]
fn collect_flat(doc: &serde_json::Value, key: &str, class: DeviceClass, out: &mut Vec<DeviceInfo>) {
    let Some(root) = doc.get(key) else { return };
    if let Some(items) = root.get("items").and_then(|v| v.as_array()) {
        for item in items {
            push_named(item, class, out);
        }
        return;
    }
    // A tree-shaped root: recurse into objects with names.
    if root.is_object() {
        walk_named(root, class, out);
    }
}

/// Depth-capped walk of the USB tree; every named node becomes a row.
#[cfg(target_os = "macos")]
fn collect_usb(doc: &serde_json::Value, key: &str, depth: usize, out: &mut Vec<DeviceInfo>) {
    if depth > USB_DEPTH_CAP {
        return;
    }
    let Some(root) = doc.get(key) else { return };
    if let Some(items) = root.get("items").and_then(|v| v.as_array()) {
        for item in items {
            push_named(item, DeviceClass::Usb, out);
            collect_children(item, DeviceClass::Usb, depth + 1, out);
        }
    } else if root.is_object() {
        walk_named(root, DeviceClass::Usb, out);
    }
}

/// Objects under the "items" key each have `_name` plus fields.
#[cfg(target_os = "macos")]
fn collect_children(
    item: &serde_json::Value,
    class: DeviceClass,
    depth: usize,
    out: &mut Vec<DeviceInfo>,
) {
    if depth > USB_DEPTH_CAP {
        return;
    }
    if let Some(children) = item.get("children").and_then(|v| v.as_array()) {
        for child in children {
            push_named(child, class, out);
            collect_children(child, class, depth + 1, out);
        }
    }
}

/// Recurse into a profiler JSON tree and push every named object.
#[cfg(target_os = "macos")]
fn walk_named(value: &serde_json::Value, class: DeviceClass, out: &mut Vec<DeviceInfo>) {
    if let serde_json::Value::Object(map) = value {
        if map.contains_key("_name") || map.contains_key("name") {
            push_named(value, class, out);
        }
        for child in map.values() {
            walk_named(child, class, out);
        }
    } else if let serde_json::Value::Array(items) = value {
        for item in items {
            walk_named(item, class, out);
        }
    }
}

#[cfg(target_os = "macos")]
fn push_named(value: &serde_json::Value, class: DeviceClass, out: &mut Vec<DeviceInfo>) {
    let Some(name) = ["_name", "name"]
        .iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    out.push(DeviceInfo {
        name: name.to_string(),
        class,
        class_raw: Some(class.label().to_string()),
        status: None,
        manufacturer: value
            .get("manufacturer")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        id: value
            .get("device_address")
            .or_else(|| value.get("serial_number"))
            .or_else(|| value.get("vendor_id"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn alsa_card_text_keeps_only_the_index_lines() {
        let text = " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n\
                     \x20                    HDA Intel PCH at 0xf7e14000 irq 123\n\
                     1 [Device          ]: USB-Audio - USB Headset\n\
                     --- no soundcards ---\n";
        let rows = super::alsa_cards_text_to_devices(text);
        assert_eq!(rows.len(), 2, "detail and placeholder lines must not count");
        assert_eq!(rows[0].name, "HDA Intel PCH");
        assert_eq!(rows[0].id.as_deref(), Some("HDA-Intel"));
        assert_eq!(rows[1].name, "USB Headset");
    }

    #[cfg(windows)]
    #[test]
    fn pnp_json_accepts_empty_single_and_array_shapes() {
        assert!(super::parse_pnp_json("").expect("ok").is_empty());
        assert!(super::parse_pnp_json("null").expect("ok").is_empty());
        let one = super::parse_pnp_json(
            "{\"Class\":\"Display\",\"FriendlyName\":\"Intel(R) UHD\",\"Status\":\"OK\",\"InstanceId\":\"PCI\\\\VEN_8086\"}",
        )
        .expect("ok");
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].class, x_core::device::DeviceClass::Display);
        let many = super::parse_pnp_json(
            "[{\"Class\":\"USB\",\"FriendlyName\":\"USB Root Hub\"},{\"Class\":\"HIDClass\",\"FriendlyName\":\"Keyboard\"}]",
        )
        .expect("ok");
        assert_eq!(many.len(), 2);
        assert_eq!(many[0].class, x_core::device::DeviceClass::Usb);
        assert_eq!(many[1].class, x_core::device::DeviceClass::Input);
    }

    #[cfg(windows)]
    #[test]
    fn pnp_rows_without_a_friendly_name_are_dropped() {
        let rows = super::parse_pnp_json("[{\"Class\":\"System\"},{\"Class\":\"Bluetooth\",\"FriendlyName\":\"Intel Wireless Bluetooth\"}]")
            .expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Intel Wireless Bluetooth");
    }
}
