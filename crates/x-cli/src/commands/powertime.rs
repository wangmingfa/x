//! `x power` and `x time`: battery, power verbs, clock facts and sync.
//!
//! Every power verb is destructive — the dispatcher routes them through the
//! shared confirmation unless `--yes` is given.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{Confirmer, OutputFormat, Renderer, Table},
    row,
};

/// `x power` subcommands.
#[derive(Debug, Subcommand)]
pub enum PowerCommand {
    /// Battery status (absent on desktops / VMs).
    Battery,

    /// Put the machine to sleep.
    Sleep {
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Shut the machine down (optionally after a delay).
    Shutdown {
        /// Delay in seconds.
        #[arg(long, default_value_t = 60)]
        delay: u32,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Restart the machine (optionally after a delay).
    Reboot {
        /// Delay in seconds.
        #[arg(long, default_value_t = 60)]
        delay: u32,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// Arguments for `x power`.
#[derive(Debug, clap::Args)]
pub struct PowerArgs {
    #[command(subcommand)]
    pub command: PowerCommand,
}

/// Route a `x power` invocation.
pub fn dispatch_power(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    command: &PowerCommand,
) -> Result<i32> {
    // Resolved lazily: `--dry-run` must print its plan even on a host that
    // has no power control at all.
    let power = || {
        context
            .power
            .as_ref()
            .ok_or_else(|| Error::unsupported("power control is not available in this context"))
    };

    match command {
        PowerCommand::Battery => {
            let battery = power()?.battery()?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&battery)?;
                return Ok(0);
            }
            if !battery.present {
                renderer.line("no battery detected")?;
                return Ok(0);
            }
            let mut table = Table::new(["field", "value"]);
            table.push(row![
                "percent",
                battery
                    .percent
                    .map(|p| format!("{p:.0}%"))
                    .unwrap_or_else(|| "-".into()),
            ]);
            table.push(row![
                "state",
                battery.state.clone().unwrap_or_else(|| "-".into()),
            ]);
            table.push(row![
                "plugged",
                battery
                    .plugged
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".into()),
            ]);
            renderer.table(&table)?;
        }
        PowerCommand::Sleep { yes } => {
            if crate::dry_run_guard(renderer, "put this machine to sleep")? {
                return Ok(0);
            }
            if !super::confirm(renderer, confirmer, *yes, "sleep")? {
                return Ok(super::EXIT_DECLINED);
            }
            power()?.sleep()?;
            renderer.line("sleep requested")?;
        }
        PowerCommand::Shutdown { delay, yes } => {
            if crate::dry_run_guard(renderer, &format!("shut this machine down in {delay}s"))? {
                return Ok(0);
            }
            if !super::confirm(renderer, confirmer, *yes, &format!("shutdown in {delay}s"))? {
                return Ok(super::EXIT_DECLINED);
            }
            power()?.shutdown(*delay)?;
            renderer.line(format!(
                "shutdown scheduled in {delay}s (cancel with the OS's own command)"
            ))?;
        }
        PowerCommand::Reboot { delay, yes } => {
            if crate::dry_run_guard(renderer, &format!("reboot this machine in {delay}s"))? {
                return Ok(0);
            }
            if !super::confirm(renderer, confirmer, *yes, &format!("reboot in {delay}s"))? {
                return Ok(super::EXIT_DECLINED);
            }
            power()?.reboot(*delay)?;
            renderer.line(format!("reboot scheduled in {delay}s"))?;
        }
    }
    Ok(0)
}

/// `x time` subcommands.
#[derive(Debug, Subcommand)]
pub enum TimeCommand {
    /// Current local and UTC time, time zone.
    Now,

    /// Just the time zone name.
    Timezone,

    /// Trigger an NTP sync via the platform's own tool.
    Sync {
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// Arguments for `x time`.
#[derive(Debug, clap::Args)]
pub struct TimeArgs {
    #[command(subcommand)]
    pub command: TimeCommand,
}

/// Route a `x time` invocation.
pub fn dispatch_time(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    command: &TimeCommand,
) -> Result<i32> {
    match command {
        TimeCommand::Now | TimeCommand::Timezone => {
            let info = x_core::power::time_info();
            let timezone = context
                .system
                .info()
                .ok()
                .and_then(|i| i.timezone)
                .unwrap_or_else(|| "unknown".to_string());
            if let TimeCommand::Timezone = command {
                renderer.line(timezone)?;
                return Ok(0);
            }
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&serde_json::json!({
                    "local": info.local,
                    "utc": info.utc,
                    "unix": info.unix_seconds,
                    "timezone": timezone,
                }))?;
                return Ok(0);
            }
            let mut table = Table::new(["field", "value"]);
            table.push(row!["local", info.local]);
            table.push(row!["utc", info.utc]);
            table.push(row!["unix", info.unix_seconds.to_string()]);
            table.push(row!["timezone", timezone]);
            renderer.table(&table)?;
        }
        TimeCommand::Sync { yes } => {
            if crate::dry_run_guard(renderer, "trigger a time sync")? {
                return Ok(0);
            }
            if !super::confirm(
                renderer,
                confirmer,
                *yes,
                "trigger a time sync (may need privileges)",
            )? {
                return Ok(super::EXIT_DECLINED);
            }
            sync_time(renderer)?;
        }
    }
    Ok(0)
}

/// One NTP sync attempt through the platform's own tool.
fn sync_time(renderer: &mut Renderer) -> Result<()> {
    let (program, args) = if cfg!(target_family = "windows") {
        ("w32tm", vec!["/resync".to_string()])
    } else if cfg!(target_os = "macos") {
        (
            "sntp",
            vec!["-sS".to_string(), "time.apple.com".to_string()],
        )
    } else {
        (
            "timedatectl",
            vec!["set-ntp".to_string(), "true".to_string()],
        )
    };
    let output = std::process::Command::new(program)
        .args(&args)
        .output()
        .map_err(|e| Error::unsupported(format!("{program} is not available: {e}")))?;
    if output.status.success() {
        renderer.line(format!("time sync requested via {program}"))?;
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let mut error = Error::system(format!(
            "{program} failed: {}",
            if stderr.is_empty() {
                "non-zero exit"
            } else {
                &stderr
            }
        ));
        error = error.with_permission(if cfg!(target_family = "windows") {
            x_core::PermissionRequirement::Administrator
        } else {
            x_core::PermissionRequirement::Root
        });
        Err(error)
    }
}
