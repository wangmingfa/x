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

/// `x container` subcommands: the same five questions `x docker` asks, plus
/// which engines this machine actually has.
#[derive(Debug, Subcommand)]
pub enum ContainerCommand {
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

    /// Which engines this machine can talk to, and which one is in use.
    Engines,
}

/// Arguments for `x container`.
#[derive(Debug, clap::Args)]
pub struct ContainerArgs {
    #[command(subcommand)]
    pub command: ContainerCommand,
}

/// An engine choice: pinned to one CLI, or left to discovery.
///
/// `x docker` pins, so it can never answer from a podman that happens to also
/// be installed; `x container` discovers.
#[derive(Debug, Clone, Copy)]
enum Choice {
    Pinned(x_core::container::Engine),
    Discover,
}

impl Choice {
    fn containers(&self, all: bool) -> Result<Vec<x_core::container::ContainerInfo>> {
        match self {
            Self::Pinned(engine) => x_core::container::containers_with(*engine, all),
            Self::Discover => x_core::container::containers(all),
        }
    }
    fn images(&self) -> Result<Vec<x_core::container::ImageInfo>> {
        match self {
            Self::Pinned(engine) => x_core::container::images_with(*engine),
            Self::Discover => x_core::container::images(),
        }
    }
    fn ports(&self) -> Result<Vec<x_core::container::ContainerPort>> {
        match self {
            Self::Pinned(engine) => x_core::container::ports_with(*engine),
            Self::Discover => x_core::container::ports(),
        }
    }
    fn logs(&self, container: &str, lines: usize) -> Result<x_core::container::ContainerLogs> {
        match self {
            Self::Pinned(engine) => x_core::container::logs_with(*engine, container, lines),
            Self::Discover => x_core::container::logs(container, lines),
        }
    }
}

/// Route an `x docker` invocation, pinned to the docker CLI.
pub fn dispatch_docker(
    _context: &SystemContext,
    renderer: &mut Renderer,
    command: &DockerCommand,
) -> Result<i32> {
    let choice = Choice::Pinned(x_core::container::Engine::Docker);
    match command {
        DockerCommand::Ps { all } => {
            render_containers(renderer, &choice.containers(*all)?)?;
        }
        DockerCommand::Images => {
            render_images(renderer, &choice.images()?)?;
        }
        DockerCommand::Ports => {
            render_ports(renderer, &choice.ports()?)?;
        }
        DockerCommand::Port { port } => render_port_owner(renderer, *port, &choice.ports()?)?,
        DockerCommand::Logs { container, lines } => {
            render_logs(renderer, &choice.logs(container, *lines)?)?;
        }
    }
    Ok(0)
}

/// Route an `x container` invocation against the engine this machine has.
pub fn dispatch_container(
    _context: &SystemContext,
    renderer: &mut Renderer,
    command: &ContainerCommand,
) -> Result<i32> {
    let choice = Choice::Discover;
    match command {
        ContainerCommand::Ps { all } => {
            render_containers(renderer, &choice.containers(*all)?)?;
        }
        ContainerCommand::Images => {
            render_images(renderer, &choice.images()?)?;
        }
        ContainerCommand::Ports => {
            render_ports(renderer, &choice.ports()?)?;
        }
        ContainerCommand::Port { port } => render_port_owner(renderer, *port, &choice.ports()?)?,
        ContainerCommand::Logs { container, lines } => {
            render_logs(renderer, &choice.logs(container, *lines)?)?;
        }
        ContainerCommand::Engines => render_engines(renderer)?,
    }
    Ok(0)
}

/// Which engines this machine can talk to, and which one is in use.
fn render_engines(renderer: &mut Renderer) -> Result<()> {
    let found = x_core::container::available_engines();
    let active = match x_core::container::active_engine() {
        Ok(engine) => Some(engine),
        // A pinned-but-unknown engine is a typo worth naming. Reporting it as
        // "no engine found" would leave the user staring at a machine that
        // plainly has one while their mistake went unmentioned.
        Err(err) if err.kind() == x_core::ErrorKind::InvalidInput => return Err(err),
        // Genuinely nothing installed: the empty listing below says so.
        Err(_) => None,
    };
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&serde_json::json!({
            "available": found,
            "active": active,
        }))?;
        return Ok(());
    }
    if found.is_empty() {
        renderer.line(
            "no container engine found: install docker, podman or nerdctl, or set X_CONTAINER_ENGINE",
        )?;
        return Ok(());
    }
    let mut table = Table::new(["engine", "cli", "active"]);
    for engine in &found {
        table.push(row![
            engine.to_string(),
            engine.cli(),
            if Some(*engine) == active { "*" } else { "" },
        ]);
    }
    renderer.table(&table)?;
    Ok(())
}

fn render_containers(
    renderer: &mut Renderer,
    rows: &[x_core::container::ContainerInfo],
) -> Result<()> {
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(());
    }
    let mut table = Table::new(["engine", "id", "image", "name", "state", "ports"]);
    for row in rows {
        table.push(row![
            row.engine.to_string(),
            row.id.clone(),
            row.image.clone(),
            row.name.clone(),
            row.state.clone(),
            row.ports.clone(),
        ]);
    }
    renderer.table(&table)?;
    Ok(())
}

fn render_images(renderer: &mut Renderer, rows: &[x_core::container::ImageInfo]) -> Result<()> {
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(());
    }
    let mut table = Table::new(["engine", "repository", "tag", "id", "size"]);
    for row in rows {
        table.push(row![
            row.engine.to_string(),
            row.repository.clone(),
            row.tag.clone(),
            row.id.clone(),
            row.size.clone(),
        ]);
    }
    renderer.table(&table)?;
    Ok(())
}

fn render_ports(renderer: &mut Renderer, rows: &[x_core::container::ContainerPort]) -> Result<()> {
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(());
    }
    let mut table = Table::new(["engine", "host", "name", "protocol"]);
    for row in rows {
        table.push(row![
            row.engine.to_string(),
            format!("{}:{}", row.host_ip, row.host_port),
            row.name.clone(),
            row.protocol.clone(),
        ]);
    }
    renderer.table(&table)?;
    Ok(())
}

fn render_port_owner(
    renderer: &mut Renderer,
    port: u16,
    rows: &[x_core::container::ContainerPort],
) -> Result<()> {
    let owner = rows.iter().find(|p| p.host_port == port);
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&owner)?;
        return Ok(());
    }
    match owner {
        Some(row) => renderer.line(format!(
            "{}:{} -> {} ({}, {})",
            row.host_ip, row.host_port, row.name, row.engine, row.protocol
        ))?,
        None => renderer.line(format!("no container publishes {port}"))?,
    }
    Ok(())
}

fn render_logs(renderer: &mut Renderer, logs: &x_core::container::ContainerLogs) -> Result<()> {
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&logs)?;
        return Ok(());
    }
    for line in &logs.lines {
        renderer.line(line.clone())?;
    }
    Ok(())
}
