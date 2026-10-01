//! `x mount`: list, inspect, mount and unmount filesystems.
//!
//! `mount` / `unmount` change the machine and usually need privileges — both
//! go through confirmation and the platform adapter reports the requirement.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{Confirmer, OutputFormat, Renderer, Table},
    row,
};

/// `x mount` subcommands.
#[derive(Debug, Subcommand)]
pub enum MountCommand {
    /// Everything currently mounted.
    List {
        /// Case insensitive substring filter on source or target.
        #[arg(long)]
        search: Option<String>,
    },

    /// One mount point in detail.
    Info {
        /// Mount point (e.g. `/`, `C:`, `D:\`).
        target: String,
    },

    /// Mount a source at a target.
    Mount {
        /// Device or share (`/dev/sdb1`, `//server/share`).
        source: String,
        /// Mount point or drive letter.
        target: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Unmount whatever is at the target.
    Unmount {
        /// Mount point or drive letter.
        target: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// Arguments for `x mount`.
#[derive(Debug, clap::Args)]
pub struct MountArgs {
    #[command(subcommand)]
    pub command: MountCommand,
}

/// Route a `x mount` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    command: &MountCommand,
) -> Result<i32> {
    let mount = context
        .mount
        .as_ref()
        .ok_or_else(|| Error::unsupported("mount management is not available in this context"))?;

    match command {
        MountCommand::List { search } => {
            let mut rows = mount.list()?;
            if let Some(needle) = search {
                let needle = needle.to_ascii_lowercase();
                rows.retain(|m| {
                    m.source.to_ascii_lowercase().contains(&needle)
                        || m.target.to_ascii_lowercase().contains(&needle)
                });
            }
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&rows)?;
                return Ok(0);
            }
            let mut table = Table::new(["source", "target", "type", "ro", "used"]);
            for m in &rows {
                let used = match (m.used_bytes, m.total_bytes) {
                    (Some(used), Some(total)) if total > 0 => {
                        format!("{:.0}%", used as f64 / total as f64 * 100.0)
                    }
                    _ => "-".to_string(),
                };
                table.push(row![
                    m.source.clone(),
                    m.target.clone(),
                    m.fs_type.clone(),
                    if m.readonly { "ro" } else { "rw" }.to_string(),
                    used,
                ]);
            }
            renderer.table(&table)?;
        }
        MountCommand::Info { target } => {
            let info = mount.info(target)?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&info)?;
                return Ok(0);
            }
            let mut table = Table::new(["field", "value"]);
            table.push(row!["source", info.source.clone()]);
            table.push(row!["target", info.target.clone()]);
            table.push(row!["type", info.fs_type.clone()]);
            table.push(row!["readonly", info.readonly.to_string()]);
            if let Some(total) = info.total_bytes {
                table.push(row!["total", x_core::format_bytes(total)]);
            }
            if let Some(used) = info.used_bytes {
                table.push(row!["used", x_core::format_bytes(used)]);
            }
            renderer.table(&table)?;
        }
        MountCommand::Mount {
            source,
            target,
            yes,
        } => {
            if !(*yes || renderer.format() == OutputFormat::Json) {
                renderer.line(format!("about to mount {source} at {target}"))?;
                if !confirmer.confirm("continue?")? {
                    return Err(Error::invalid_input("aborted by user"));
                }
            }
            mount.mount(source, target)?;
            renderer.line(format!("mounted {source} at {target}"))?;
        }
        MountCommand::Unmount { target, yes } => {
            if !(*yes || renderer.format() == OutputFormat::Json) {
                renderer.line(format!("about to unmount {target}"))?;
                if !confirmer.confirm("continue?")? {
                    return Err(Error::invalid_input("aborted by user"));
                }
            }
            mount.unmount(target)?;
            renderer.line(format!("unmounted {target}"))?;
        }
    }
    Ok(0)
}
