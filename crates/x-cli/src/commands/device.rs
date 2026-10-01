//! `x device`: the machine's hardware inventory, read-only.
//!
//! One enumeration surface over `Get-PnpDevice`, sysfs and `system_profiler`.
//! Class filtering happens here in the frontend, so the adapter can stay a
//! single round trip to the platform's own tool.

use clap::{Args, Subcommand};
use x_core::device::DeviceClass;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::format::{OutputFormat, Renderer, Table};
use crate::row;

/// `x device` subcommands.
#[derive(Debug, Subcommand)]
pub enum DeviceCommand {
    /// Every present device, optionally filtered by class.
    List(ListArgs),
    /// USB devices (`x device list --class usb`).
    Usb,
    /// Bluetooth controllers and radios.
    Bluetooth,
    /// Sound cards and audio endpoints.
    Audio,
    /// Display adapters and connectors.
    Display,
    /// Cameras.
    Camera,
    /// Keyboards, mice and other HID endpoints.
    Input,
    /// Network interfaces at the hardware level.
    Network,
}

/// Arguments for `x device list`.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Only devices of this class.
    #[arg(long, value_name = "CLASS")]
    class: Option<String>,
}

/// Route a `x device` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &DeviceCommand,
) -> Result<i32> {
    let wanted = match command {
        DeviceCommand::List(args) => parse_class(args.class.as_deref())?,
        DeviceCommand::Usb => Some(DeviceClass::Usb),
        DeviceCommand::Bluetooth => Some(DeviceClass::Bluetooth),
        DeviceCommand::Audio => Some(DeviceClass::Audio),
        DeviceCommand::Display => Some(DeviceClass::Display),
        DeviceCommand::Camera => Some(DeviceClass::Camera),
        DeviceCommand::Input => Some(DeviceClass::Input),
        DeviceCommand::Network => Some(DeviceClass::Network),
    };

    let manager = context
        .device
        .as_ref()
        .ok_or_else(|| Error::unsupported("no device enumerator in this context"))?;
    let mut devices = manager.devices()?;
    if let Some(class) = wanted {
        devices.retain(|device| device.class == class);
    }
    devices.sort_by(|a, b| (a.class.label(), &a.name).cmp(&(b.class.label(), &b.name)));

    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&devices)?;
        return Ok(0);
    }
    if devices.is_empty() {
        let scope = match wanted {
            Some(class) => format!(" of class {}", class.label()),
            None => String::new(),
        };
        renderer.line(format!("no present devices{scope}"))?;
        return Ok(0);
    }

    let mut table = Table::new(["class", "name", "status", "manufacturer", "id"]);
    for device in &devices {
        table.push(row![
            device.class.label(),
            device.name.clone(),
            device.status.clone().unwrap_or_else(|| "-".into()),
            device.manufacturer.clone().unwrap_or_else(|| "-".into()),
            device.id.clone().unwrap_or_else(|| "-".into()),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// Parse `--class` strictly: the normalized set only, no fuzzy guessing.
fn parse_class(token: Option<&str>) -> Result<Option<DeviceClass>> {
    let Some(token) = token else { return Ok(None) };
    let t = token.trim().to_ascii_lowercase();
    let found = match t.as_str() {
        "usb" => DeviceClass::Usb,
        "bluetooth" => DeviceClass::Bluetooth,
        "audio" => DeviceClass::Audio,
        "display" => DeviceClass::Display,
        "camera" => DeviceClass::Camera,
        "input" => DeviceClass::Input,
        "network" => DeviceClass::Network,
        "other" => DeviceClass::Other,
        _ => {
            return Err(Error::invalid_input(format!(
                "unknown device class {token:?}; try usb, bluetooth, audio, display, camera, input, network, other"
            )))
        }
    };
    Ok(Some(found))
}
