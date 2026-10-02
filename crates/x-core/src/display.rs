//! Displays and monitors: one read-only view over the platform's display topology.
//!
//! Windows walks the display devices with `EnumDisplayDevicesW` /
//! `EnumDisplaySettingsW` and asks each monitor its effective DPI; Linux
//! reads DRM connector attributes in sysfs (`status`, `modes`,
//! `dpms_power`); macOS parses `system_profiler SPDisplaysDataType -json`.
//! Whatever a platform cannot answer — compositor-only facts like position
//! on Wayland, scale on X11 — stays `None` rather than being guessed.
//!
//! Everything here is a read: no confirmation, no elevation, no audit.

use serde::{Deserialize, Serialize};

/// Pixel size of a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    /// Horizontal pixels.
    pub width: u32,
    /// Vertical pixels.
    pub height: u32,
}

/// Position of the display's top-left corner in desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    /// X coordinate; negative for monitors left of the primary one.
    pub x: i32,
    /// Y coordinate.
    pub y: i32,
}

/// One monitor (or built-in panel) as the platform sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayInfo {
    /// Human name: monitor device string, connector name, panel model.
    pub name: String,
    /// Platform identifier (`\\.\DISPLAY1`, `HDMI-A-1`, panel serial…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Cable attached, when the platform distinguishes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected: Option<bool>,
    /// Active mode size, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
    /// Vertical refresh of the active mode, in Hz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_hz: Option<f64>,
    /// Effective UI scaling, 100 meaning 1×.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_percent: Option<u32>,
    /// This is the primary display, when the platform says so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<bool>,
    /// Top-left corner in desktop coordinates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Point>,
    /// High dynamic range state, where the platform reports it.
    ///
    /// `Some(true)` means HDR is active right now, `Some(false)` that the
    /// display can do it but is not in that mode, and `None` that nothing on
    /// this platform or this machine says — which is not the same as `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hdr_enabled: Option<bool>,
}

impl DisplayInfo {
    /// A display that only has a name so far.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            id: None,
            connected: None,
            resolution: None,
            refresh_hz: None,
            scale_percent: None,
            primary: None,
            position: None,
            hdr_enabled: None,
        }
    }
}

/// Display topology reads.
pub trait DisplayManager: Send + Sync {
    /// Every display the platform reports, in platform order.
    fn displays(&self) -> crate::error::Result<Vec<DisplayInfo>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_serialize_only_what_is_known() {
        let mut info = DisplayInfo::new("HDMI-A-1");
        info.resolution = Some(Resolution {
            width: 1920,
            height: 1080,
        });
        let json = serde_json::to_string(&info).expect("json");
        assert_eq!(
            json,
            "{\"name\":\"HDMI-A-1\",\"resolution\":{\"width\":1920,\"height\":1080}}"
        );
    }

    #[test]
    fn round_trip_keeps_the_topology() {
        let mut info = DisplayInfo::new("Internal");
        info.primary = Some(true);
        info.position = Some(Point { x: 0, y: 0 });
        info.scale_percent = Some(150);
        let back: DisplayInfo =
            serde_json::from_str(&serde_json::to_string(&info).expect("json")).expect("back");
        assert_eq!(back, info);
    }

    #[test]
    fn an_unknown_hdr_state_is_absent_rather_than_false() {
        // "nothing said" and "HDR is off" are different claims, so an unknown
        // state must not serialize as `false`.
        let info = DisplayInfo::new("HDMI-A-1");
        let json = serde_json::to_string(&info).expect("json");
        assert!(!json.contains("hdr"), "{json}");
    }

    #[test]
    fn hdr_state_survives_a_round_trip_in_both_polarities() {
        for enabled in [true, false] {
            let mut info = DisplayInfo::new("DELL U2720Q");
            info.hdr_enabled = Some(enabled);
            let back: DisplayInfo =
                serde_json::from_str(&serde_json::to_string(&info).expect("json")).expect("back");
            assert_eq!(back.hdr_enabled, Some(enabled));
        }
    }
}
