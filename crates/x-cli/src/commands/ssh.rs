//! `x ssh`: hosts from `~/.ssh/config`, reachability probes and connections.

use clap::Subcommand;
use std::time::Duration;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x ssh` subcommands.
#[derive(Debug, Subcommand)]
pub enum SshCommand {
    /// Hosts defined in `~/.ssh/config`, plus known_hosts and keys.
    Hosts,

    /// Resolve a host the way `ssh -G` would, without connecting.
    Test {
        /// Alias or pattern from the config.
        alias: String,
    },

    /// TCP reachability probe against the resolved host:port.
    Ping {
        /// Alias from the config, or a literal `host` / `host:port`.
        target: String,
        /// Connection timeout in seconds.
        #[arg(long, default_value_t = 5)]
        timeout: u64,
    },

    /// Start an interactive SSH session (blocks until logout).
    Connect {
        /// Alias or user@host to connect to.
        target: String,
    },
}

/// Arguments for `x ssh`.
#[derive(Debug, clap::Args)]
pub struct SshArgs {
    #[command(subcommand)]
    pub command: SshCommand,
}

/// Route a `x ssh` invocation.
pub fn dispatch(
    _context: &SystemContext,
    renderer: &mut Renderer,
    command: &SshCommand,
) -> Result<i32> {
    match command {
        SshCommand::Hosts => {
            hosts(renderer)?;
        }
        SshCommand::Test { alias } => {
            let host = x_core::sshcfg::test(alias)?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&host)?;
                return Ok(0);
            }
            let mut table = Table::new(["field", "value"]);
            table.push(row!["alias", host.name.clone()]);
            table.push(row![
                "hostname",
                host.hostname.clone().unwrap_or_else(|| "-".into()),
            ]);
            table.push(row![
                "user",
                host.user.clone().unwrap_or_else(|| "-".into()),
            ]);
            table.push(row![
                "port",
                host.port
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "22".into()),
            ]);
            table.push(row!["identity", host.identity_files.join(", ")]);
            table.push(row!["target", host.target()]);
            renderer.table(&table)?;
        }
        SshCommand::Ping { target, timeout } => {
            let (host, port) = resolve_target(target)?;
            let elapsed = x_core::sshcfg::ping(&host, port, Duration::from_secs(*timeout))?;
            renderer.line(format!(
                "{host}:{port} reachable in {:.0} ms",
                elapsed.as_millis()
            ))?;
        }
        SshCommand::Connect { target } => {
            x_core::sshcfg::connect(target)?;
        }
    }
    Ok(0)
}

fn hosts(renderer: &mut Renderer) -> Result<i32> {
    let home = home_dir()?;
    let overview = x_core::sshcfg::overview(&home);
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&overview)?;
        return Ok(0);
    }
    let mut table = Table::new(["host", "target", "identity"]);
    for host in &overview.hosts {
        table.push(row![
            host.name.clone(),
            host.target(),
            host.identity_files.join(", "),
        ]);
    }
    renderer.table(&table)?;
    renderer.line(format!(
        "{} known_hosts entries; keys: {}",
        overview.known_hosts,
        if overview.keys.is_empty() {
            "none".to_string()
        } else {
            overview.keys.join(", ")
        }
    ))?;
    Ok(0)
}

/// `alias` resolves through the config; `host:port` and bare `host` are literal.
fn resolve_target(target: &str) -> Result<(String, u16)> {
    if let Some((host, port)) = target.rsplit_once(':') {
        if let Ok(port) = port.parse() {
            return Ok((host.to_string(), port));
        }
    }
    // Not host:port — try the config first so `x ssh ping web` uses HostName/Port.
    if let Ok(resolved) = x_core::sshcfg::test(target) {
        if let Some(hostname) = resolved.hostname {
            return Ok((hostname, resolved.port.unwrap_or(22)));
        }
    }
    Ok((target.to_string(), 22))
}

fn home_dir() -> Result<std::path::PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| Error::unsupported("cannot determine the home directory"))
}
