//! `x dev`, `x doctor`, `x project`, `x docker`: the developer toolbelt.
//!
//! All four are thin views over pure `x-core` modules — no platform adapter
//! involved, and every failure becomes a row or an honest error.

use clap::Subcommand;
use x_core::error::Result;
use x_core::SystemContext;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x dev` arguments: one optional tool narrows the sweep.
#[derive(Debug, clap::Args)]
pub struct DevArgs {
    /// Only report one tool (`node`, `cargo`, ...).
    pub tool: Option<String>,
}

/// Route a `x dev` invocation.
pub fn dispatch_dev(
    _context: &SystemContext,
    renderer: &mut Renderer,
    args: &DevArgs,
) -> Result<i32> {
    let tools = match &args.tool {
        Some(name) => vec![x_core::devcheck::check_tool(name)],
        None => x_core::devcheck::dev_overview(),
    };
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&tools)?;
        return Ok(0);
    }
    let mut table = Table::new(["tool", "installed", "path", "version"]);
    for tool in &tools {
        table.push(row![
            tool.name.clone(),
            tool.mark().to_string(),
            tool.path.clone().unwrap_or_else(|| "-".into()),
            tool.version.clone().unwrap_or_else(|| "-".into()),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// `x doctor` arguments.
#[derive(Debug, clap::Args)]
pub struct DoctorArgs {
    /// Command to health-check (`x doctor node`). Without it, sweep the whole
    /// environment (network, DNS, proxy, toolchains).
    pub command: Option<String>,
}

/// Route a `x doctor` invocation.
pub fn dispatch_doctor(
    _context: &SystemContext,
    renderer: &mut Renderer,
    args: &DoctorArgs,
) -> Result<i32> {
    match &args.command {
        Some(name) => {
            let check = x_core::devcheck::doctor_command(name);
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&check)?;
                return Ok(if check.verdict == "ok" { 0 } else { 1 });
            }
            renderer.line(format!(
                "{} {} — {}",
                check.tool.mark(),
                check.tool.name,
                check.verdict
            ))?;
            if let Some(path) = &check.tool.path {
                renderer.line(format!("path: {path}"))?;
            }
            if let Some(version) = &check.tool.version {
                renderer.line(format!("version: {version}"))?;
            }
            Ok(if check.verdict == "ok" { 0 } else { 1 })
        }
        None => {
            let rows = x_core::devcheck::doctor_system();
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&rows)?;
                return Ok(0);
            }
            let mut table = Table::new(["area", "status", "detail"]);
            for check in &rows {
                table.push(row![
                    check.area.clone(),
                    check.mark.clone(),
                    check.detail.clone(),
                ]);
            }
            renderer.table(&table)?;
            Ok(0)
        }
    }
}

/// `x project` arguments.
#[derive(Debug, clap::Args)]
pub struct ProjectArgs {
    /// Directory to inspect; defaults to the current one.
    pub path: Option<String>,
}

/// Route a `x project` invocation.
pub fn dispatch_project(
    _context: &SystemContext,
    renderer: &mut Renderer,
    args: &ProjectArgs,
) -> Result<i32> {
    let dir = args.path.clone().unwrap_or_else(|| ".".into());
    let info = x_core::project::detect(dir.as_ref());
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&info)?;
        return Ok(0);
    }
    let mut table = Table::new(["field", "value"]);
    table.push(row!["root", info.root.display().to_string()]);
    table.push(row![
        "kinds",
        if info.kinds.is_empty() {
            "-".into()
        } else {
            info.kinds.join(", ")
        }
    ]);
    table.push(row![
        "package_manager",
        info.package_manager
            .map(|m| m.name().to_string())
            .unwrap_or_else(|| "-".into()),
    ]);
    table.push(row!["name", info.name.unwrap_or_else(|| "-".into()),]);
    table.push(row!["version", info.version.unwrap_or_else(|| "-".into()),]);
    table.push(row!["branch", info.branch.unwrap_or_else(|| "-".into()),]);
    table.push(row!["entry_points", info.entry_points.join(", ")]);
    renderer.table(&table)?;
    Ok(0)
}

/// `x docker` subcommands.
#[derive(Debug, Subcommand)]
pub enum DockerCommand {
    /// Containers (`-a` includes stopped ones).
    Ps {
        /// Include stopped containers.
        #[arg(long, short = 'a')]
        all: bool,
    },

    /// Images, newest first.
    Images,

    /// Every published host port across running containers.
    Ports,

    /// Which container publishes a host port.
    Port {
        /// Host port number.
        port: u16,
    },

    /// Recent log lines of one container.
    Logs {
        /// Container name or id.
        container: String,
        /// Maximum number of lines.
        #[arg(long, value_name = "N", default_value_t = 50)]
        lines: usize,
    },
}

/// Arguments for `x docker`.
#[derive(Debug, clap::Args)]
pub struct DockerArgs {
    #[command(subcommand)]
    pub command: DockerCommand,
}

/// Route a `x docker` invocation.
pub fn dispatch_docker(
    _context: &SystemContext,
    renderer: &mut Renderer,
    command: &DockerCommand,
) -> Result<i32> {
    match command {
        DockerCommand::Ps { all } => {
            let containers = x_core::dockerinfo::containers(*all)?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&containers)?;
                return Ok(0);
            }
            let mut table = Table::new(["id", "image", "name", "state", "ports"]);
            for c in &containers {
                table.push(row![
                    c.id.clone(),
                    c.image.clone(),
                    c.name.clone(),
                    c.state.clone(),
                    c.ports.clone(),
                ]);
            }
            renderer.table(&table)?;
        }
        DockerCommand::Images => {
            let images = x_core::dockerinfo::images()?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&images)?;
                return Ok(0);
            }
            let mut table = Table::new(["repository", "tag", "id", "size"]);
            for image in &images {
                table.push(row![
                    image.repository.clone(),
                    image.tag.clone(),
                    image.id.clone(),
                    image.size.clone(),
                ]);
            }
            renderer.table(&table)?;
        }
        DockerCommand::Ports => {
            let ports = x_core::dockerinfo::ports()?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&ports)?;
                return Ok(0);
            }
            let mut table = Table::new(["host", "container", "name", "protocol"]);
            for p in &ports {
                table.push(row![
                    format!("{}:{}", p.host_ip, p.host_port),
                    p.container.clone(),
                    p.name.clone(),
                    p.protocol.clone(),
                ]);
            }
            renderer.table(&table)?;
        }
        DockerCommand::Port { port } => {
            let owner = x_core::dockerinfo::port_owner(*port)?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&owner)?;
                return Ok(0);
            }
            match owner {
                Some(p) => renderer.line(format!(
                    "{}:{} -> {} ({})",
                    p.host_ip, p.host_port, p.name, p.container
                ))?,
                None => renderer.line(format!("no container publishes {port}"))?,
            }
        }
        DockerCommand::Logs { container, lines } => {
            let logs = x_core::dockerinfo::logs(container, *lines)?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&logs)?;
                return Ok(0);
            }
            for line in &logs.lines {
                renderer.line(line.clone())?;
            }
        }
    }
    Ok(0)
}
