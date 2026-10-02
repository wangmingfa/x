//! Linux and macOS display topology.
//!
//! - **Linux** — DRM connector attributes in sysfs: `status` says whether
//!   a cable is attached, the first line of `modes` is the active mode
//!   (`1920x1080 60.00 …`). Position, primary and scaling live in the
//!   compositor, not the kernel, so they stay absent rather than guessed.
//! - **macOS** — `system_profiler SPDisplaysDataType -json`: every display
//!   under a GPU's `spdisplays_ndrvs` list, with the pixel resolution and
//!   the UI size when the panel is scaled.
//!
//! Everything is a read: no confirmation, no elevation, no audit.

#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use x_core::display::Resolution;
use x_core::display::{DisplayInfo, DisplayManager};
#[cfg(any(windows, target_os = "macos"))]
use x_core::error::Error;
use x_core::error::Result;

/// Adapter installed into the composition root (Linux / macOS; Windows uses
/// the native Gdi walk in `windows::display`).
#[derive(Debug, Default)]
pub struct PlatformDisplays;

impl DisplayManager for PlatformDisplays {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        #[cfg(target_os = "linux")]
        {
            Ok(linux_displays())
        }
        #[cfg(target_os = "macos")]
        {
            macos_displays()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(Error::unsupported(
                "no display enumerator is compiled in for this platform",
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Linux: /sys/class/drm connectors
// ---------------------------------------------------------------------------

/// Walk every DRM connector directory (`card0-HDMI-A-1`, `card0-DP-1`, …).
#[cfg(target_os = "linux")]
fn linux_displays() -> Vec<DisplayInfo> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let Some(connector) = connector_name(&entry.file_name().to_string_lossy()) else {
            continue;
        };
        let mut info = DisplayInfo::new(connector.clone());
        info.id = Some(connector);
        info.connected = read_trim(&dir.join("status")).map(|s| s == "connected");
        let modes = std::fs::read_to_string(dir.join("modes")).unwrap_or_default();
        if let Some((resolution, refresh)) = parse_modes(&modes) {
            info.resolution = Some(resolution);
            info.refresh_hz = refresh;
        }
        out.push(info);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// `card0-HDMI-A-1` → `HDMI-A-1`; `card0` / `renderD128` → `None`.
#[cfg(target_os = "linux")]
pub fn connector_name(entry: &str) -> Option<String> {
    let (card, connector) = entry.split_once('-')?;
    if card.is_empty() || connector.is_empty() || !card.starts_with("card") {
        return None;
    }
    Some(connector.to_string())
}

/// First `modes` line: `1920x1080 60.00 59.94 …` → size and the mode's
/// vertical refresh. An empty file is a disabled connector, not an error.
#[cfg(target_os = "linux")]
pub fn parse_modes(text: &str) -> Option<(Resolution, Option<f64>)> {
    let first = text.lines().next()?.trim();
    let mut tokens = first.split_whitespace();
    let size = tokens.next()?;
    let (width, height) = size.split_once('x')?;
    let resolution = Resolution {
        width: width.parse().ok()?,
        height: height.parse().ok()?,
    };
    let refresh = tokens.next().and_then(|t| t.parse::<f64>().ok());
    Some((resolution, refresh))
}

#[cfg(target_os = "linux")]
fn read_trim(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

// ---------------------------------------------------------------------------
// macOS: system_profiler SPDisplaysDataType
// ---------------------------------------------------------------------------

/// One `system_profiler SPDisplaysDataType -json` document.
#[cfg(target_os = "macos")]
fn macos_displays() -> Result<Vec<DisplayInfo>> {
    let output = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
        .map_err(|e| Error::system(format!("cannot run system_profiler: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(Error::system(if stderr.is_empty() {
            format!(
                "system_profiler SPDisplaysDataType failed (exit {:?})",
                output.status.code()
            )
        } else {
            format!("system_profiler SPDisplaysDataType failed: {stderr}")
        }));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let doc: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| Error::system(format!("cannot parse SPDisplaysDataType JSON: {e}")))?;
    Ok(parse_macos_displays(&doc))
}

/// Every display under every GPU's `spdisplays_ndrvs` list.
#[cfg(target_os = "macos")]
pub fn parse_macos_displays(doc: &serde_json::Value) -> Vec<DisplayInfo> {
    let Some(root) = doc.get("SPDisplaysDataType") else {
        return Vec::new();
    };
    let gpus: Vec<&serde_json::Value> = match root {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(_) => vec![root],
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for gpu in gpus {
        let Some(displays) = gpu.get("spdisplays_ndrvs").and_then(|v| v.as_array()) else {
            continue;
        };
        for display in displays {
            if let Some(info) = macos_display(display) {
                out.push(info);
            }
        }
    }
    out
}

/// One `spdisplays_ndrvs` entry.
#[cfg(target_os = "macos")]
fn macos_display(display: &serde_json::Value) -> Option<DisplayInfo> {
    let name = display
        .get("_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;
    let mut info = DisplayInfo::new(name);
    // `spdisplays_resolution` reads like `1920 x 1080 (as 960 x 540)`;
    // `spdisplays_pixels` like `1440 x 900`. Prefer the pixel form's first
    // pair as the native resolution.
    let pixels = display.get("spdisplays_pixels").and_then(|v| v.as_str());
    let resolution_text = display
        .get("spdisplays_resolution")
        .and_then(|v| v.as_str());
    let (native, looks_like) = match (pixels, resolution_text) {
        (Some(pixels), _) => (
            first_size(pixels),
            second_size(resolution_text.unwrap_or("")),
        ),
        (None, Some(text)) => {
            let (first, second) = (first_size(text), second_size(text));
            (first.or(second), second)
        }
        (None, None) => (None, None),
    };
    if let Some(native) = native {
        info.resolution = Some(native);
        if let Some(looks_like) = looks_like {
            if looks_like.width > 0 && looks_like.width != native.width {
                info.scale_percent = Some(
                    ((native.width as f64) / (looks_like.width as f64) * 100.0).round() as u32,
                );
            }
        }
    }
    info.primary = display
        .get("spdisplays_main")
        .and_then(|v| v.as_str())
        .map(|s| s.to_ascii_lowercase().ends_with("yes"));
    // A display appearing in the list is attached; there is no detached form.
    info.connected = Some(true);
    Some(info)
}

/// First `1234 x 567` pair in the text.
#[cfg(target_os = "macos")]
pub fn first_size(text: &str) -> Option<Resolution> {
    size_at(text, 0)
}

/// The pair that follows ` as ` — the UI size of a scaled panel.
#[cfg(target_os = "macos")]
pub fn second_size(text: &str) -> Option<Resolution> {
    // The profiler writes `1920 x 1080 (as 960 x 540)`; prose without the
    // parenthesis also occurs, so accept both separators.
    let (_, after) = text
        .split_once(" (as ")
        .or_else(|| text.split_once(" as "))?;
    size_at(after, 0)
}

/// Parse the `index`-th `W x H` pair out of `text`.
#[cfg(target_os = "macos")]
fn size_at(text: &str, index: usize) -> Option<Resolution> {
    let bytes = text.as_bytes();
    let mut found = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let w_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let width: u32 = text[w_start..i].parse().ok()?;
        // `W x H` may be spaced (`1920 x 1080`) or tight (`1920x1080`).
        let mut j = i;
        while j < bytes.len() && bytes[j] == b' ' {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b'x' {
            continue;
        }
        let mut k = j + 1;
        while k < bytes.len() && bytes[k] == b' ' {
            k += 1;
        }
        let h_start = k;
        while k < bytes.len() && bytes[k].is_ascii_digit() {
            k += 1;
        }
        if k == h_start {
            continue;
        }
        let height: u32 = text[h_start..k].parse().ok()?;
        if found == index {
            return Some(Resolution { width, height });
        }
        found += 1;
        i = k;
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn connector_names_keep_only_real_connectors() {
        assert_eq!(
            super::connector_name("card0-HDMI-A-1").as_deref(),
            Some("HDMI-A-1")
        );
        assert_eq!(super::connector_name("card0"), None);
        assert_eq!(super::connector_name("renderD128"), None);
        assert_eq!(super::connector_name("version"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn modes_parse_size_and_refresh_from_the_first_line() {
        let text = "1920x1080 60.00 59.94 50.00\n1280x720 60.00\n";
        let (resolution, refresh) = super::parse_modes(text).expect("mode");
        assert_eq!(resolution.width, 1920);
        assert_eq!(resolution.height, 1080);
        assert_eq!(refresh, Some(60.0));
        assert_eq!(super::parse_modes(""), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_displays_parse_pixels_and_scaled_ui_size() {
        let doc = serde_json::json!({"SPDisplaysDataType": [{
            "_name": "Apple M1 Pro",
            "spdisplays_ndrvs": [
                {
                    "_name": "Color LCD",
                    "spdisplays_pixels": "3456 x 2234",
                    "spdisplays_resolution": "1728 x 1117 (as 1728 x 1117)",
                    "spdisplays_main": "spdisplays_yes"
                },
                {
                    "_name": "DELL U2720Q",
                    "spdisplays_resolution": "3840 x 2160 (as 1920 x 1080)",
                    "spdisplays_main": "spdisplays_no"
                }
            ]
        }]});
        let rows = super::parse_macos_displays(&doc);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Color LCD");
        assert_eq!(rows[0].resolution.expect("resolution").width, 3456);
        assert_eq!(rows[0].primary, Some(true));
        assert_eq!(rows[1].resolution.expect("resolution").width, 3840);
        assert_eq!(rows[1].scale_percent, Some(200));
        assert_eq!(rows[1].primary, Some(false));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn size_pairs_stop_at_the_first_match() {
        assert_eq!(super::first_size("as 960 x 540 (x)").expect("w").width, 960);
        assert_eq!(
            super::second_size("1920 x 1080 as 960 x 540")
                .expect("w")
                .width,
            960
        );
        assert_eq!(super::second_size("1920 x 1080"), None);
        assert_eq!(super::first_size("no digits here"), None);
    }
}
