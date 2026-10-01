//! Bluetooth: local radios, paired devices, and the verbs that move state.
//!
//! Capability across platforms is uneven and we say so: Linux has
//! `bluetoothctl`, macOS publishes inventory through `system_profiler` but
//! pairs only in the GUI, Windows enumerates PnP Bluetooth objects. Reads go
//! wherever the platform allows; `scan` / `connect` / `disconnect` default to
//! [`crate::error::ErrorKind::Unsupported`] and only a backend that can drive
//! a real stack overrides them.

use serde::{Deserialize, Serialize};

/// One local Bluetooth controller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BluetoothAdapter {
    /// Controller name as the platform prints it.
    pub name: String,
    /// Controller MAC, when the platform reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Radio power, when the platform reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub powered: Option<bool>,
    /// Platform state text ("On", "Running", "OK"), verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Manufacturer / chip text, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
}

/// A Bluetooth device the platform knows about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BluetoothDevice {
    /// Device name, when the platform or the device itself supplied one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Device address, when the platform exposes it. Required for the verbs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Paired with this machine, when the platform distinguishes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paired: Option<bool>,
    /// Currently connected, when the platform distinguishes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected: Option<bool>,
    /// Signal strength in dBm during the last scan, when measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rssi: Option<i16>,
    /// Platform identifier (PnP instance, controller interface, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Normalize a user-typed Bluetooth address: six colon-separated hex pairs,
/// rendered uppercase. Anything else is rejected rather than passed along —
/// `bluetoothctl connect garbage` fails in someone else's vocabulary.
pub fn normalize_address(raw: &str) -> crate::error::Result<String> {
    let trimmed = raw.trim().to_ascii_uppercase();
    let parts: Vec<&str> = trimmed.split(':').collect();
    if parts.len() != 6
        || parts
            .iter()
            .any(|p| p.len() != 2 || !p.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(crate::Error::invalid_input(format!(
            "{raw:?} is not a Bluetooth address; expected form AA:BB:CC:DD:EE:FF"
        )));
    }
    Ok(trimmed)
}

/// Bluetooth inspection and control.
pub trait BluetoothManager: Send + Sync {
    /// Local controllers.
    fn adapters(&self) -> crate::error::Result<Vec<BluetoothAdapter>>;

    /// Devices the platform already knows (paired or enumerated).
    fn devices(&self) -> crate::error::Result<Vec<BluetoothDevice>>;

    /// Discover nearby devices for the given time. Backends without a
    /// non-interactive discovery path leave this unsupported.
    fn scan(&self, _timeout_ms: u64) -> crate::error::Result<Vec<BluetoothDevice>> {
        Err(crate::Error::unsupported(
            "this platform has no non-interactive Bluetooth scan",
        ))
    }

    /// Connect to `address` (normalized MAC). State change: confirm and audit.
    fn connect(&self, _address: &str) -> crate::error::Result<()> {
        Err(crate::Error::unsupported(
            "this platform cannot connect Bluetooth without a GUI",
        ))
    }

    /// Disconnect `address`. State change: confirm and audit.
    fn disconnect(&self, _address: &str) -> crate::error::Result<()> {
        Err(crate::Error::unsupported(
            "this platform cannot disconnect Bluetooth without a GUI",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_normalize_to_uppercase_colons() {
        assert_eq!(
            normalize_address("aa:bb:cc:dd:ee:ff").expect("ok"),
            "AA:BB:CC:DD:EE:FF"
        );
        assert_eq!(
            normalize_address(" 0a:1B:2c:3D:4e:5F ").expect("ok"),
            "0A:1B:2C:3D:4E:5F"
        );
    }

    #[test]
    fn wrong_shapes_are_rejected() {
        for bad in [
            "",
            "aabbccddeeff",
            "aa:bb:cc:dd:ee",
            "aa:bb:cc:dd:ee:ff:00",
            "aa:bb:cc:dd:ee:gg",
            "zz:bb:cc:dd:ee:ff",
        ] {
            assert!(normalize_address(bad).is_err(), "{bad:?} must not pass");
        }
    }

    #[test]
    fn devices_serialize_only_what_is_known() {
        let device = BluetoothDevice {
            name: Some("WH-1000XM3".into()),
            address: None,
            paired: None,
            connected: None,
            rssi: None,
            id: None,
        };
        assert_eq!(
            serde_json::to_string(&device).expect("json"),
            "{\"name\":\"WH-1000XM3\"}"
        );
    }
}
