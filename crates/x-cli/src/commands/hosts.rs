//! `x hosts`: inspect and edit the hosts file.
//!
//! Reads are plain; `add` / `remove` rewrite the file, which needs write
//! access to `/etc/hosts` (or the Windows equivalent) — without elevation the
//! OS error surfaces as a permission error with guidance.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x hosts` subcommands.
#[derive(Debug, Subcommand)]
pub enum HostsCommand {
    /// Every entry in the hosts file.
    List,

    /// Resolve a name through the hosts file (first match wins).
    Get {
        /// Host name to look up.
        name: String,
    },

    /// Add a mapping, or move existing names to a new IP.
    Add {
        /// IP address.
        ip: String,
        /// One or more host names.
        names: Vec<String>,
        /// Trailing comment for the new line.
        #[arg(long)]
        comment: Option<String>,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Remove every entry that carries the name.
    Remove {
        /// Host name to remove.
        name: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Print the hosts file location.
    Path,
}

/// Arguments for `x hosts`.
#[derive(Debug, clap::Args)]
pub struct HostsArgs {
    #[command(subcommand)]
    pub command: HostsCommand,
}

/// Route a `x hosts` invocation.
pub fn dispatch(
    _context: &SystemContext,
    renderer: &mut Renderer,
    command: &HostsCommand,
) -> Result<i32> {
    let path = x_platform::common::hosts_os::default_path();
    match command {
        HostsCommand::Path => {
            renderer.line(path.display().to_string())?;
        }
        HostsCommand::List => {
            let hosts = load(&path)?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&hosts.entries)?;
                return Ok(0);
            }
            let mut table = Table::new(["ip", "names", "comment", "line"]);
            for entry in &hosts.entries {
                table.push(row![
                    entry.ip.clone(),
                    entry.names.join(" "),
                    entry.comment.clone().unwrap_or_else(|| "-".into()),
                    entry.line.to_string(),
                ]);
            }
            renderer.table(&table)?;
        }
        HostsCommand::Get { name } => {
            let hosts = load(&path)?;
            match x_core::hostsfile::lookup(&hosts, name) {
                Some(entry) => {
                    if renderer.format() == OutputFormat::Json {
                        renderer.always_json(entry)?;
                        return Ok(0);
                    }
                    renderer.line(format!("{} {}", entry.ip, entry.names.join(" ")))?;
                }
                None => {
                    renderer.line(format!("{name}: not in {path:?}"))?;
                    return Ok(1);
                }
            }
        }
        HostsCommand::Add {
            ip,
            names,
            comment,
            yes,
        } => {
            if names.is_empty() {
                return Err(Error::invalid_input("at least one host name is required"));
            }
            let hosts = load(&path)?;
            let new_text = x_core::hostsfile::add(&hosts, ip, names, comment.as_deref());
            if crate::dry_run_guard(
                renderer,
                &format!("add {} -> {ip} to {}", names.join(", "), path.display()),
            )? {
                return Ok(0);
            }
            confirm_write(renderer, *yes, &path, &new_text)?;
            renderer.line(format!("added {} -> {ip}", names.join(", ")))?;
        }
        HostsCommand::Remove { name, yes } => {
            let hosts = load(&path)?;
            let Some(new_text) = x_core::hostsfile::remove(&hosts, name) else {
                renderer.line(format!("{name}: not in {path:?}"))?;
                return Ok(1);
            };
            if crate::dry_run_guard(renderer, &format!("remove {name} from {}", path.display()))? {
                return Ok(0);
            }
            confirm_write(renderer, *yes, &path, &new_text)?;
            renderer.line(format!("removed {name}"))?;
        }
    }
    Ok(0)
}

fn load(path: &std::path::Path) -> Result<x_core::hostsfile::HostsFile> {
    x_core::hostsfile::load(path)
        .map_err(|e| Error::not_found(format!("cannot read {}: {e}", path.display())))
}

fn confirm_write(
    renderer: &mut Renderer,
    yes: bool,
    path: &std::path::Path,
    new_text: &str,
) -> Result<()> {
    renderer.line(format!("about to rewrite {}", path.display()))?;
    if yes {
        return write(path, new_text);
    }
    let mut stdin_confirmer = crate::format::StdinConfirmer;
    if crate::format::Confirmer::confirm(&mut stdin_confirmer, "continue?")? {
        write(path, new_text)
    } else {
        Err(Error::invalid_input("aborted by user"))
    }
}

fn write(path: &std::path::Path, text: &str) -> Result<()> {
    std::fs::write(path, text).map_err(|e| {
        let mut error = Error::system(format!("cannot write {}: {e}", path.display()));
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            // Writing the hosts file is an administrator / root operation on
            // every supported OS; say so instead of a bare EPERM.
            error = error.with_permission(if cfg!(target_family = "windows") {
                x_core::PermissionRequirement::Administrator
            } else {
                x_core::PermissionRequirement::Root
            });
        }
        error
    })
}
