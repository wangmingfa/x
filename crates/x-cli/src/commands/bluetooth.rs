//! `x bluetooth`: local radios, known devices, and the link verbs.
//!
//! Reads pass through untouched. `scan` / `connect` / `disconnect` exist as
//! commands on every platform but only succeed where the OS offers a
//! non-interactive path (Linux with bluetoothctl); elsewhere the adapter
//! answers Unsupported (exit 7) instead of pretending. The verbs change
//! state, so they are confirmed and audited like the firewall rules.

use clap::Subcommand;
use x_core::bluetooth::{normalize_address, BluetoothDevice, BluetoothManager};
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{Confirmer, OutputFormat, Renderer, Table},
    row,
};

/// Discovery window when `--timeout` is not given.
const DEFAULT_SCAN_SECONDS: u64 = 10;

/// `x bluetooth` subcommands.
#[derive(Debug, Subcommand)]
pub enum BluetoothCommand {
    /// The radios this machine owns.
    Adapters,

    /// Devices the platform already knows (paired or enumerated).
    Devices,

    /// Discover nearby devices for a while (Linux only).
    Scan {
        /// Discovery window in seconds.
        #[arg(long, default_value_t = DEFAULT_SCAN_SECONDS)]
        timeout: u64,
    },

    /// Connect a known device by MAC (Linux only).
    Connect(LinkArgs),

    /// Drop the link to a device (Linux only).
    Disconnect(LinkArgs),
}

/// Arguments shared by `connect` and `disconnect`.
#[derive(Debug, clap::Args)]
pub struct LinkArgs {
    /// Device address, e.g. `80:A9:CD:54:6B:81`.
    pub address: String,
    /// Do not ask for confirmation.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for `x bluetooth`.
#[derive(Debug, clap::Args)]
pub struct BluetoothArgs {
    #[command(subcommand)]
    pub command: BluetoothCommand,
}

/// Route a `x bluetooth` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    command: &BluetoothCommand,
) -> Result<i32> {
    let bluetooth = context
        .bluetooth
        .as_ref()
        .ok_or_else(|| Error::unsupported("no bluetooth capability in this context"))?;

    match command {
        BluetoothCommand::Adapters => {
            let rows = bluetooth.adapters()?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&rows)?;
                return Ok(0);
            }
            if rows.is_empty() {
                renderer.line("no bluetooth adapters found")?;
                return Ok(0);
            }
            let mut table = Table::new(["name", "address", "powered", "state", "manufacturer"]);
            for adapter in &rows {
                table.push(row![
                    adapter.name.clone(),
                    adapter.address.clone().unwrap_or_else(|| "-".into()),
                    yes_no(adapter.powered),
                    adapter.state.clone().unwrap_or_else(|| "-".into()),
                    adapter.manufacturer.clone().unwrap_or_else(|| "-".into()),
                ]);
            }
            renderer.table(&table)?;
        }
        BluetoothCommand::Devices => {
            render_devices(
                renderer,
                &bluetooth.devices()?,
                "no bluetooth devices known to the platform",
            )?;
        }
        BluetoothCommand::Scan { timeout } => {
            let rows = bluetooth.scan(timeout * 1000)?;
            render_devices(renderer, &rows, "no devices discovered in the last scan")?;
        }
        BluetoothCommand::Connect(args) => {
            let address = verb(renderer, confirmer, bluetooth.as_ref(), args, "connect")?;
            renderer.line(format!("connecting {address}"))?;
        }
        BluetoothCommand::Disconnect(args) => {
            let address = verb(renderer, confirmer, bluetooth.as_ref(), args, "disconnect")?;
            renderer.line(format!("disconnecting {address}"))?;
        }
    }
    Ok(0)
}

/// Validate, confirm, run one link verb. The audit decorator records the
/// outcome — including a refusal by the platform.
fn verb(
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    bluetooth: &dyn BluetoothManager,
    args: &LinkArgs,
    action: &str,
) -> Result<String> {
    let address = normalize_address(&args.address)?;
    if !args.yes && renderer.format() != OutputFormat::Json {
        renderer.line(format!("about to {action} {address}"))?;
        if !confirmer.confirm("continue?")? {
            return Err(Error::invalid_input("aborted by user"));
        }
    }
    match action {
        "connect" => bluetooth.connect(&address)?,
        _ => bluetooth.disconnect(&address)?,
    }
    Ok(address)
}

/// The device table, shared by `devices` and `scan`.
fn render_devices(renderer: &mut Renderer, rows: &[BluetoothDevice], empty: &str) -> Result<()> {
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(());
    }
    if rows.is_empty() {
        renderer.line(empty)?;
        return Ok(());
    }
    let mut table = Table::new(["name", "address", "paired", "connected", "rssi"]);
    for device in rows {
        table.push(row![
            device.name.clone().unwrap_or_else(|| "-".into()),
            device.address.clone().unwrap_or_else(|| "-".into()),
            yes_no(device.paired),
            yes_no(device.connected),
            device
                .rssi
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ]);
    }
    renderer.table(&table)?;
    Ok(())
}

/// Honest three-state rendering: a platform that does not report a flag
/// shows `-`, not `no`.
fn yes_no(value: Option<bool>) -> String {
    match value {
        Some(true) => "yes".to_string(),
        Some(false) => "no".to_string(),
        None => "-".to_string(),
    }
}
