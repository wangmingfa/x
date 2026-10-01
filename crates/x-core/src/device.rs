//! Hardware device inventory: one read-only view over three platform stores.
//!
//! Windows enumerates PnP devices (`Get-PnpDevice`), Linux walks the
//! `/sys/class` trees, macOS flattens `system_profiler -json` documents.
//! The platform's own class and status vocabulary is preserved in
//! `class_raw` / `status`; the normalized [`DeviceClass`] only ever says
//! what the raw token clearly means.
//!
//! Everything here is a read: no confirmation, no elevation, no audit.

use serde::{Deserialize, Serialize};

/// Normalized device category. Adapters derive it from the platform's own
/// class token and keep that token in [`DeviceInfo::class_raw`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceClass {
    /// USB bus devices and hubs.
    Usb,
    /// Bluetooth controllers and radios.
    Bluetooth,
    /// Sound cards and audio endpoints.
    Audio,
    /// Displays and monitors.
    Display,
    /// Cameras.
    Camera,
    /// Keyboards, mice and other HID endpoints.
    Input,
    /// Network interfaces at the hardware level.
    Network,
    /// Anything the adapter could not bucket.
    Other,
}

impl DeviceClass {
    /// Lowercase token accepted on the command line and in JSON.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Usb => "usb",
            Self::Bluetooth => "bluetooth",
            Self::Audio => "audio",
            Self::Display => "display",
            Self::Camera => "camera",
            Self::Input => "input",
            Self::Network => "network",
            Self::Other => "other",
        }
    }

    /// Bucket a platform class token ("USB", "HDA-Intel", "hid") without
    /// inventing detail: unknown tokens stay [`Self::Other`].
    pub fn parse(token: &str) -> Self {
        let t = token.trim().to_ascii_lowercase();
        if t.contains("usb") {
            Self::Usb
        } else if t.contains("bluetooth") || t == "bt" {
            Self::Bluetooth
        } else if t.contains("audio")
            || t.contains("media")
            || t.contains("sound")
            || t.contains("hda")
        {
            Self::Audio
        } else if t.contains("display") || t.contains("monitor") || t.contains("gpu") {
            Self::Display
        } else if t.contains("camera") || t.contains("imaging") {
            Self::Camera
        } else if t.contains("keyboard")
            || t.contains("mouse")
            || t.contains("hid")
            || t.contains("input")
        {
            Self::Input
        } else if t.contains("net") || t.contains("ethernet") || t.contains("wi-fi") {
            Self::Network
        } else {
            Self::Other
        }
    }
}

/// One device as the platform reported it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Friendly / product name.
    pub name: String,
    /// Normalized category.
    pub class: DeviceClass,
    /// The platform's own class token, kept verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_raw: Option<String>,
    /// Platform status text ("OK", "Error", "connected"), verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Manufacturer / provider text, when the platform reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    /// Stable platform identifier (PnP instance id, sysfs path, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl DeviceInfo {
    /// Build a device with only the parts the platform actually reported.
    pub fn new(name: impl Into<String>, class: DeviceClass) -> Self {
        Self {
            name: name.into(),
            class,
            class_raw: None,
            status: None,
            manufacturer: None,
            id: None,
        }
    }
}

/// Read-only hardware enumeration.
pub trait DeviceManager: Send + Sync {
    /// Every present device the platform can name.
    fn devices(&self) -> crate::error::Result<Vec<DeviceInfo>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_tokens_bucket_only_what_they_mean() {
        assert_eq!(DeviceClass::parse("USB"), DeviceClass::Usb);
        assert_eq!(DeviceClass::parse("Bluetooth"), DeviceClass::Bluetooth);
        assert_eq!(DeviceClass::parse("Media"), DeviceClass::Audio);
        assert_eq!(DeviceClass::parse("HDA-Intel"), DeviceClass::Audio);
        assert_eq!(DeviceClass::parse("Display"), DeviceClass::Display);
        assert_eq!(DeviceClass::parse("Camera"), DeviceClass::Camera);
        assert_eq!(DeviceClass::parse("HIDClass"), DeviceClass::Input);
        assert_eq!(DeviceClass::parse("NET"), DeviceClass::Network);
        // Unknown tokens are not guessed into a familiar bucket.
        assert_eq!(DeviceClass::parse("1394"), DeviceClass::Other);
        assert_eq!(DeviceClass::parse(""), DeviceClass::Other);
    }

    #[test]
    fn devices_serialize_minimally() {
        let dev = DeviceInfo::new("USB2.0 Camera", DeviceClass::Camera);
        assert_eq!(
            serde_json::to_string(&dev).expect("json"),
            "{\"name\":\"USB2.0 Camera\",\"class\":\"camera\"}"
        );
    }
}
