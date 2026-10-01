//! Cross-platform Bluetooth.
//!
//! The three platforms differ in how much they let a CLI touch, and the
//! adapter reports exactly that:
//!
//! - **Windows** — `Get-PnpDevice -Class Bluetooth` as one JSON document.
//!   Radios are the rows whose instance path is not `BTHENUM\*` (those are
//!   remotely enumerated devices). Windows exposes no PowerShell verb to
//!   connect without the GUI, so the state-changing verbs stay unsupported.
//! - **Linux** — `bluetoothctl`, the BlueZ frontend itself: `list` and
//!   `show` for controllers, `devices` (plus the `Paired` / `Connected`
//!   filters, which older builds lack — then those flags stay absent),
//!   `--timeout N scan on`, and `connect` / `disconnect` for the verbs.
//!   Without bluetoothctl installed the reads fail honestly.
//! - **macOS** — `system_profiler SPBluetoothDataType -json`. The
//!   controller block gives address and on/off; the device list marks
//!   paired/connected per entry. Pairing is GUI-only, so verbs stay
//!   unsupported there too.
//!
//! Reads are never audited; the verbs audit at the decorator layer.

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
use std::process::Command;

use x_core::bluetooth::{BluetoothAdapter, BluetoothDevice, BluetoothManager};
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
use x_core::error::Error;
use x_core::error::Result;

/// Adapter installed into the composition root.
#[derive(Debug, Default)]
pub struct PlatformBluetooth;

impl BluetoothManager for PlatformBluetooth {
    fn adapters(&self) -> Result<Vec<BluetoothAdapter>> {
        #[cfg(windows)]
        {
            windows_adapters()
        }
        #[cfg(target_os = "linux")]
        {
            linux_adapters()
        }
        #[cfg(target_os = "macos")]
        {
            macos_adapters()
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            Err(Error::unsupported(
                "no Bluetooth enumerator is compiled in for this platform",
            ))
        }
    }

    fn devices(&self) -> Result<Vec<BluetoothDevice>> {
        #[cfg(windows)]
        {
            windows_devices()
        }
        #[cfg(target_os = "linux")]
        {
            linux_devices()
        }
        #[cfg(target_os = "macos")]
        {
            macos_devices()
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            Err(Error::unsupported(
                "no Bluetooth enumerator is compiled in for this platform",
            ))
        }
    }

    #[cfg(target_os = "linux")]
    fn scan(&self, timeout_ms: u64) -> Result<Vec<BluetoothDevice>> {
        bluetoothctl(&["--timeout", &timeout_ms.to_string(), "scan", "on"])?;
        // Discovery results join the adapter's device cache, which is what
        // `devices` prints; RSSI is not persisted there, so rows may lack it.
        linux_devices()
    }

    #[cfg(target_os = "linux")]
    fn connect(&self, address: &str) -> Result<()> {
        bluetoothctl_verb(&["connect", address])
    }

    #[cfg(target_os = "linux")]
    fn disconnect(&self, address: &str) -> Result<()> {
        bluetoothctl_verb(&["disconnect", address])
    }
}

// ---------------------------------------------------------------------------
// Windows: Get-PnpDevice -Class Bluetooth
// ---------------------------------------------------------------------------

/// One PS projection shared by adapters and devices.
#[cfg(windows)]
const PNP_BLUETOOTH_SCRIPT: &str = "$ErrorActionPreference='Continue';\
    Get-PnpDevice -Class Bluetooth -PresentOnly -ErrorAction SilentlyContinue \
    | Select-Object FriendlyName,Manufacturer,Status,InstanceId \
    | ConvertTo-Json -Compress -Depth 2";

/// Instance rows plus a `BTHENUM` split: radios vs. remotely known devices.
#[cfg(windows)]
#[derive(Debug)]
pub struct PnpBtRow {
    pub name: String,
    pub manufacturer: Option<String>,
    pub status: Option<String>,
    pub instance: Option<String>,
}

#[cfg(windows)]
fn pnp_rows() -> Result<Vec<PnpBtRow>> {
    let text = run_powershell_json(PNP_BLUETOOTH_SCRIPT)?;
    Ok(parse_pnp_bt(&text))
}

#[cfg(windows)]
fn windows_adapters() -> Result<Vec<BluetoothAdapter>> {
    Ok(pnp_rows()?
        .iter()
        .filter(|row| !is_bthenum(row))
        .map(|row| BluetoothAdapter {
            name: row.name.clone(),
            address: None,
            powered: None,
            state: row.status.clone(),
            manufacturer: row.manufacturer.clone(),
        })
        .collect())
}

#[cfg(windows)]
fn windows_devices() -> Result<Vec<BluetoothDevice>> {
    Ok(pnp_rows()?
        .iter()
        .filter(|row| is_bthenum(row))
        .map(|row| BluetoothDevice {
            name: Some(row.name.clone()),
            address: address_from_instance(row.instance.as_deref().unwrap_or("")),
            paired: None,
            connected: None,
            rssi: None,
            id: row.instance.clone(),
        })
        .collect())
}

/// `BTHENUM\…` rows are devices enumerated through a radio, not radios.
#[cfg(windows)]
fn is_bthenum(row: &PnpBtRow) -> bool {
    row.instance
        .as_deref()
        .is_some_and(|id| id.to_ascii_uppercase().starts_with("BTHENUM\\"))
}

/// Instance paths like `BTHENUM\{class}_{12-hex-mac}_LOCALMFG&…` carry the
/// device address. No match means no address, never a guess.
#[cfg(windows)]
pub fn address_from_instance(instance: &str) -> Option<String> {
    let hex = instance
        .split(['_', '&'])
        .find(|part| part.len() == 12 && part.bytes().all(|b| b.is_ascii_hexdigit()))?;
    let upper = hex.to_ascii_uppercase();
    let mut out = String::with_capacity(17);
    for (i, chunk) in upper.as_bytes().chunks(2).enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&String::from_utf8_lossy(chunk));
    }
    Some(out)
}

#[cfg(windows)]
pub fn parse_pnp_bt(text: &str) -> Vec<PnpBtRow> {
    let text = text.trim();
    if text.is_empty() || text == "null" {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let rows = match value {
        serde_json::Value::Array(rows) => rows,
        other => vec![other],
    };
    rows.iter()
        .filter_map(|row| {
            let name = row.get("FriendlyName").and_then(|v| v.as_str())?;
            if name.is_empty() {
                return None;
            }
            Some(PnpBtRow {
                name: name.to_string(),
                manufacturer: field_str(row, "Manufacturer"),
                status: field_str(row, "Status"),
                instance: field_str(row, "InstanceId"),
            })
        })
        .collect()
}

#[cfg(windows)]
fn field_str(row: &serde_json::Value, key: &str) -> Option<String> {
    row.get(key)
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
// Linux: bluetoothctl
// ---------------------------------------------------------------------------

/// Run bluetoothctl with fixed arguments; missing tool = honest unsupported.
#[cfg(target_os = "linux")]
fn bluetoothctl(args: &[&str]) -> Result<String> {
    let output = Command::new("bluetoothctl")
        .arg("--no-color")
        .args(args)
        .output()
        .map_err(|_| {
            Error::unsupported("bluetoothctl is not available; install BlueZ to use x bluetooth")
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(Error::system(if stderr.is_empty() {
            format!(
                "bluetoothctl {} failed (exit {:?})",
                args.first().copied().unwrap_or(""),
                output.status.code()
            )
        } else {
            format!("bluetoothctl failed: {stderr}")
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A state-changing verb: exit status alone is not enough — bluetoothctl
/// prints `Failed: …` and still returns 0 in non-interactive mode.
#[cfg(target_os = "linux")]
fn bluetoothctl_verb(args: &[&str]) -> Result<()> {
    let text = bluetoothctl(args)?;
    if let Some(line) = text
        .lines()
        .find(|line| line.trim_start().starts_with("Failed:"))
    {
        return Err(Error::new(
            x_core::error::ErrorKind::InvalidState,
            line.trim().to_string(),
        ));
    }
    Ok(())
}

/// `Controller <mac> <name> [default]` lines → adapters, each refined by
/// `show <mac>` for the powered flag.
#[cfg(target_os = "linux")]
fn linux_adapters() -> Result<Vec<BluetoothAdapter>> {
    let listed = parse_controllers(&bluetoothctl(&["list"])?);
    let mut out = Vec::with_capacity(listed.len());
    for mut adapter in listed {
        if let Some(address) = adapter.address.clone() {
            // `show` on one dead controller should not sink the whole list.
            if let Ok(text) = bluetoothctl(&["show", &address]) {
                adapter.powered = parse_powered(&text);
            }
        }
        out.push(adapter);
    }
    Ok(out)
}

/// `bluetoothctl devices` plus the filter queries a platform may not have.
#[cfg(target_os = "linux")]
fn linux_devices() -> Result<Vec<BluetoothDevice>> {
    let mut rows = parse_devices(&bluetoothctl(&["devices"])?);
    let indexed: std::collections::HashMap<String, usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, row)| row.address.clone().map(|a| (a, i)))
        .collect();
    for (flag, field) in [("Paired", true), ("Connected", false)] {
        // Older bluetoothctl rejects the filter argument: degrade to absent.
        let Ok(text) = bluetoothctl(&["devices", flag]) else {
            continue;
        };
        for found in parse_devices(&text) {
            let Some(address) = found.address else {
                continue;
            };
            if let Some(&i) = indexed.get(&address) {
                if field {
                    rows[i].paired = Some(true);
                } else {
                    rows[i].connected = Some(true);
                }
            }
        }
    }
    // Devices not listed by either filter keep `None`: unknown, not false.
    Ok(rows)
}

/// `Controller XX:…:XX Name [default]` — verbatim name, no default marker guess.
#[cfg(target_os = "linux")]
pub fn parse_controllers(text: &str) -> Vec<BluetoothAdapter> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("Controller ")?;
            let mut words = rest.splitn(3, ' ');
            let address = words.next()?.to_ascii_uppercase();
            let tail = words.next()?;
            // The trailing `[default]` tag belongs to the name column.
            let name = tail
                .trim_end_matches(" [default]")
                .trim_end_matches("[default]")
                .trim()
                .to_string();
            if name.is_empty() {
                return None;
            }
            Some(BluetoothAdapter {
                name,
                address: Some(address),
                powered: None,
                state: None,
                manufacturer: None,
            })
        })
        .collect()
}

/// `Powered: yes|no` from `show`; anything else is not reported.
#[cfg(target_os = "linux")]
pub fn parse_powered(text: &str) -> Option<bool> {
    text.lines().find_map(|line| {
        let value = line
            .trim()
            .strip_prefix("Powered:")?
            .trim()
            .to_ascii_lowercase();
        match value.as_str() {
            "yes" => Some(true),
            "no" => Some(false),
            _ => None,
        }
    })
}

/// `Device XX:…:XX Name…` lines.
#[cfg(target_os = "linux")]
pub fn parse_devices(text: &str) -> Vec<BluetoothDevice> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("Device ") else {
            continue;
        };
        let mut words = rest.splitn(2, ' ');
        let (Some(address), Some(name)) = (words.next(), words.next()) else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        out.push(BluetoothDevice {
            name: Some(name.to_string()),
            address: Some(address.to_ascii_uppercase()),
            paired: None,
            connected: None,
            rssi: None,
            id: None,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// macOS: system_profiler SPBluetoothDataType
// ---------------------------------------------------------------------------

/// The whole Bluetooth document, parsed once per read.
#[cfg(target_os = "macos")]
fn profiler_bluetooth() -> Result<serde_json::Value> {
    let output = Command::new("system_profiler")
        .args(["SPBluetoothDataType", "-json"])
        .output()
        .map_err(|e| Error::system(format!("cannot run system_profiler: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(Error::system(if stderr.is_empty() {
            format!(
                "system_profiler SPBluetoothDataType failed (exit {:?})",
                output.status.code()
            )
        } else {
            format!("system_profiler SPBluetoothDataType failed: {stderr}")
        }));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .map_err(|e| Error::system(format!("cannot parse SPBluetoothDataType: {e}")))
}

#[cfg(target_os = "macos")]
fn macos_adapters() -> Result<Vec<BluetoothAdapter>> {
    let doc = profiler_bluetooth()?;
    let root = doc.get("SPBluetoothDataType");
    Ok(parse_macos_adapters(root))
}

/// `controller_properties` is the one local radio macOS reports.
#[cfg(target_os = "macos")]
pub fn parse_macos_adapters(root: Option<&serde_json::Value>) -> Vec<BluetoothAdapter> {
    let Some(props) = root.and_then(|r| r.get("controller_properties")) else {
        return Vec::new();
    };
    let name = props
        .get("name_of_controller")
        .and_then(|v| v.as_str())
        .unwrap_or("Bluetooth")
        .to_string();
    let device_field = |key: &str| {
        props
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    vec![BluetoothAdapter {
        name,
        address: device_field("address"),
        powered: device_field("device_status").map(|s| s.eq_ignore_ascii_case("On")),
        state: device_field("device_status"),
        manufacturer: device_field("vendor_id"),
    }]
}

#[cfg(target_os = "macos")]
fn macos_devices() -> Result<Vec<BluetoothDevice>> {
    let doc = profiler_bluetooth()?;
    let root = doc.get("SPBluetoothDataType");
    Ok(parse_macos_devices(root))
}

/// `device_list` holds one object per device, keyed by the device name.
/// Older layouts nest under category arrays — both shapes are walked.
#[cfg(target_os = "macos")]
pub fn parse_macos_devices(root: Option<&serde_json::Value>) -> Vec<BluetoothDevice> {
    let Some(list) = root.and_then(|r| r.get("device_list")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_device_objects(list, &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Visit every `{name: {fields}}` pairing in the device list, whichever
/// shape the OS release produced.
#[cfg(target_os = "macos")]
fn collect_device_objects(value: &serde_json::Value, out: &mut Vec<BluetoothDevice>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, entry) in map {
                if key == "_name" {
                    continue;
                }
                match entry {
                    serde_json::Value::Object(fields) => push_macos_device(key, fields, out),
                    serde_json::Value::Array(items) => {
                        for item in items {
                            collect_device_objects(item, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_device_objects(item, out);
            }
        }
        _ => {}
    }
}

/// One device entry: the key is its name, the object its flags.
#[cfg(target_os = "macos")]
fn push_macos_device(
    name: &str,
    fields: &serde_json::Map<String, serde_json::Value>,
    out: &mut Vec<BluetoothDevice>,
) {
    let yes_no = |key: &str| {
        fields
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.eq_ignore_ascii_case("yes"))
    };
    let address = fields
        .get("address")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if name == "unknown_device" && address.is_none() {
        return;
    }
    out.push(BluetoothDevice {
        name: (!name.is_empty()).then(|| name.to_string()),
        address,
        paired: yes_no("paired"),
        connected: yes_no("connected"),
        rssi: fields
            .get("rssi")
            .and_then(|v| v.as_str())
            .and_then(parse_rssi),
        id: None,
    });
}

/// RSSI is printed like `-54 (0xca)`; the leading number is the value.
#[cfg(target_os = "macos")]
pub fn parse_rssi(text: &str) -> Option<i16> {
    let token = text.split_whitespace().next()?;
    token.parse::<i16>().ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn radios_and_devices_split_on_instance_namespace() {
        let json = r#"[
            {"FriendlyName":"Intel(R) Wireless Bluetooth","Manufacturer":"Intel Corporation","Status":"OK","InstanceId":"USB\\VID_8087&PID_0026\\5&1234&0&14"},
            {"FriendlyName":"WH-1000XM3","Status":"OK","InstanceId":"BTHENUM\\{0000110B-0000-1000-8000-00805F9B34FB}_80A9CD546B81_0"},
            {"Status":"OK"}
        ]"#;
        let rows = super::parse_pnp_bt(json);
        assert_eq!(rows.len(), 2, "rows without a friendly name are dropped");
        let adapters: Vec<_> = rows.iter().filter(|r| !super::is_bthenum(r)).collect();
        let devices: Vec<_> = rows.iter().filter(|r| super::is_bthenum(r)).collect();
        assert_eq!(adapters.len(), 1);
        assert_eq!(devices.len(), 1);
        assert_eq!(
            super::address_from_instance(devices[0].instance.as_deref().unwrap_or("")).as_deref(),
            Some("80:A9:CD:54:6B:81")
        );
        // A device enumerated by the LOCALMFG placeholder has no address.
        assert_eq!(
            super::address_from_instance(
                r"BTHENUM\{00001101-0000-1000-8000-00805f9b34fb}_LOCALMFG&0000"
            ),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn pnp_bluetooth_accepts_empty_single_and_null_shapes() {
        assert!(super::parse_pnp_bt("").is_empty());
        assert!(super::parse_pnp_bt("null").is_empty());
        assert_eq!(
            super::parse_pnp_bt("{\"FriendlyName\":\"Radio\",\"InstanceId\":\"USB\\\\x\"}").len(),
            1
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn controller_lines_keep_the_verbatim_name() {
        let text = "Controller 0A:1B:2C:3D:4E:5F my-machine [default]\nController AA:BB:CC:DD:EE:FF dongle\n";
        let rows = super::parse_controllers(text);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "my-machine");
        assert_eq!(rows[0].address.as_deref(), Some("0A:1B:2C:3D:4E:5F"));
        assert_eq!(rows[1].name, "dongle");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn powered_is_read_from_show_output_only_when_yes_or_no() {
        assert_eq!(super::parse_powered("Powered: yes\n"), Some(true));
        assert_eq!(super::parse_powered("  Powered: no"), Some(false));
        assert_eq!(super::parse_powered("Discovering: on"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn device_lines_split_address_from_the_rest_of_the_name() {
        let text = "Device 80:A9:cd:54:6b:81 Sony WH-1000XM3\nnot a device line\nDevice FF:00\n";
        let rows = super::parse_devices(text);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].address.as_deref(), Some("80:A9:CD:54:6B:81"));
        assert_eq!(rows[0].name.as_deref(), Some("Sony WH-1000XM3"));
        assert_eq!(rows[0].paired, None, "unknown is not false");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn profiler_controller_maps_status_to_powered() {
        let doc: serde_json::Value = serde_json::from_str(
            r#"{"controller_properties":{"address":"ac:de:48:00:11:22","device_status":"On","vendor_id":"Apple (0x5ac.0x8295)"}}"#,
        )
        .expect("json");
        let rows = super::parse_macos_adapters(doc.get("SPBluetoothDataType"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].powered, Some(true));
        assert_eq!(rows[0].address.as_deref(), Some("ac:de:48:00:11:22"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn profiler_devices_survive_both_layouts() {
        let array_doc = serde_json::json!({"device_list":[
            {"WH-1000XM3":{"address":"80:a9:cd:54:6b:81","connected":"Yes","paired":"Yes","rssi":"-54 (0xca)"}},
            {"Keyboard":{"address":"aa:bb:cc:dd:ee:ff","connected":"No","paired":"Yes"}}
        ]});
        let map_doc = serde_json::json!({"device_list":{
            "WH-1000XM3":{"address":"80:a9:cd:54:6b:81","connected":"Yes"}
        }});
        let rows = super::parse_macos_devices(Some(&array_doc));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name.as_deref(), Some("Keyboard"), "sorted by name");
        assert_eq!(rows[0].connected, Some(false));
        assert_eq!(rows[0].paired, Some(true));
        assert_eq!(rows[1].name.as_deref(), Some("WH-1000XM3"));
        assert_eq!(rows[1].rssi, Some(-54));
        assert_eq!(super::parse_macos_devices(Some(&map_doc)).len(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rssi_keeps_the_leading_number() {
        assert_eq!(super::parse_rssi("-54 (0xca)"), Some(-54));
        assert_eq!(super::parse_rssi("not a number"), None);
    }
}
