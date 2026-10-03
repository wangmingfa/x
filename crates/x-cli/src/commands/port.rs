//! `x port`: sockets, owners and the kill plan.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::format_bytes;
use x_core::port::{
    diff_sockets, summarize, ConnectionState, KillPlan, PortInfo, PortListOptions, PortOwner,
    PortQuery, PortSort, Protocol,
};
use x_core::SystemContext;

use crate::SignalArg;
use crate::{
    format::{Confirmer, OutputFormat, Renderer, Table},
    row,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `x port`: filters apply to the implicit `list`, subcommands do the rest.
#[derive(Debug, clap::Args)]
pub struct PortArgs {
    /// Subcommand. Omitted means "list listening sockets".
    #[command(subcommand)]
    pub command: Option<PortCommand>,
    /// A bare port number means "who owns this, and which project is it".
    #[arg(value_name = "PORT")]
    pub port: Option<u16>,
    /// Filters, only used without a subcommand.
    #[command(flatten)]
    pub list: PortListArgs,
}

/// `x port` subcommands.
#[derive(Debug, Subcommand)]
pub enum PortCommand {
    /// List sockets, listening ones by default.
    List(PortListArgs),

    /// Every socket, not only listening ones.
    All(PortListArgs),

    /// Group sockets by the process owning them.
    Owners(PortListArgs),

    /// Count sockets by state and protocol, plus kernel queue occupancy.
    Stats(PortListArgs),

    /// Exit 0 when something holds the port, 1 when it is free.
    Check {
        /// Port number to probe.
        port: u16,
    },

    /// Show what killing a port would do, then do it.
    Kill {
        /// Port to free.
        port: u16,
        /// Signal used to terminate the owners.
        #[arg(long, value_enum, default_value_t = SignalArg::Term)]
        signal: SignalArg,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Free every socket held by processes whose name matches.
    KillByName {
        /// Process name substring.
        name: String,
        /// Signal used to terminate the owners.
        #[arg(long, value_enum, default_value_t = SignalArg::Term)]
        signal: SignalArg,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Sockets held by processes whose name matches (read-only counterpart of kill-by-name).
    Find {
        /// Process name substring.
        name: String,
    },

    /// Poll sockets and print only what changed between polls.
    Watch {
        /// Filters, same as `x port all` but defaulting to all states.
        #[command(flatten)]
        list: PortListArgs,
        /// Poll interval in seconds.
        #[arg(long, default_value_t = 2.0)]
        interval: f64,
        /// Stop after this many polls instead of running until Ctrl-C.
        #[arg(long)]
        count: Option<usize>,
    },
}

/// Shared filters for the listing commands.
#[derive(Debug, clap::Args)]
pub struct PortListArgs {
    /// Only this protocol.
    #[arg(long = "proto", value_enum)]
    pub protocol: Option<ProtocolArg>,
    /// Only sockets in this state.
    #[arg(long, value_enum)]
    pub state: Option<StateArg>,
    /// Only listening sockets.
    #[arg(long)]
    pub listen: bool,
    /// Case insensitive substring of port, process name or address.
    #[arg(long)]
    pub search: Option<String>,
    /// Maximum number of rows.
    #[arg(long)]
    pub limit: Option<usize>,
}

/// Protocol filter values.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ProtocolArg {
    /// TCP.
    Tcp,
    /// UDP.
    Udp,
    /// Unix domain socket.
    Unix,
}

impl From<ProtocolArg> for Protocol {
    fn from(value: ProtocolArg) -> Self {
        match value {
            ProtocolArg::Tcp => Protocol::Tcp,
            ProtocolArg::Udp => Protocol::Udp,
            ProtocolArg::Unix => Protocol::Unix,
        }
    }
}

/// Connection state filter values.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum StateArg {
    /// Accepting connections.
    Listen,
    /// Connection established.
    Established,
    /// Waiting to be reused.
    TimeWait,
    /// Closed.
    Closed,
    /// Datagram socket bound without a peer.
    Bound,
}

impl From<StateArg> for ConnectionState {
    fn from(value: StateArg) -> Self {
        match value {
            StateArg::Listen => ConnectionState::Listen,
            StateArg::Established => ConnectionState::Established,
            StateArg::TimeWait => ConnectionState::TimeWait,
            StateArg::Closed => ConnectionState::Closed,
            StateArg::Bound => ConnectionState::Bound,
        }
    }
}

impl PortListArgs {
    fn options(&self, listening_default: bool) -> PortListOptions {
        PortListOptions {
            protocol: self.protocol.map(Into::into),
            state: self.state.map(Into::into),
            listening_only: self.listen || listening_default,
            search: self.search.clone(),
            limit: self.limit,
        }
    }
}

/// Route a `x port` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    args: &PortArgs,
) -> Result<i32> {
    let Some(command) = &args.command else {
        if let Some(port) = args.port {
            return who(context, renderer, port);
        }
        return list(context, renderer, &args.list.options(true));
    };
    match command {
        PortCommand::List(args) => list(context, renderer, &args.options(true)),
        PortCommand::All(args) => list(context, renderer, &args.options(false)),
        PortCommand::Owners(args) => owners(context, renderer, &args.options(true)),
        PortCommand::Stats(args) => stats(context, renderer, &args.options(false)),
        PortCommand::Check { port } => check(context, renderer, *port),
        PortCommand::Kill { port, signal, yes } => kill(
            context,
            renderer,
            confirmer,
            PortQuery::Port(*port),
            (*signal).into(),
            *yes,
        ),
        PortCommand::KillByName { name, signal, yes } => kill(
            context,
            renderer,
            confirmer,
            PortQuery::Process(name.clone()),
            (*signal).into(),
            *yes,
        ),
        PortCommand::Find { name } => find(context, renderer, name),
        PortCommand::Watch {
            list,
            interval,
            count,
        } => watch(context, renderer, &list.options(false), *interval, *count),
    }
}

/// List sockets, optionally as one row per owning process.
pub fn list(
    context: &SystemContext,
    renderer: &mut Renderer,
    options: &PortListOptions,
) -> Result<i32> {
    let rows = context.port.list(options)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
    } else {
        let mut table = socket_table();
        for row in &rows {
            table.push(socket_row(row));
        }
        renderer.table(&table)?;
    }
    Ok(0)
}

/// Group sockets by owning process.
pub fn owners(
    context: &SystemContext,
    renderer: &mut Renderer,
    options: &PortListOptions,
) -> Result<i32> {
    let groups: Vec<PortOwner> = context.port.owners(options)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&groups)?;
    } else {
        let mut table = Table::new(["pid", "process", "ports"]);
        for owner in &groups {
            let ports = owner
                .port_numbers()
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(",");
            table.push(row![
                owner.pid.to_string(),
                owner.process_name.clone().unwrap_or_else(unknown),
                ports,
            ]);
        }
        renderer.table(&table)?;
    }
    Ok(0)
}

/// Probe a port.
///
/// Exit code 0 means "something holds this port" and 1 means "free": the same
/// convention as `pgrep` and `nc -z`, so `if x port check 3000` reads naturally.
pub fn check(context: &SystemContext, renderer: &mut Renderer, port: u16) -> Result<i32> {
    let in_use = !context.port.is_free(port)?;
    let owners = if in_use {
        context.port.find_port(port)?
    } else {
        Vec::new()
    };

    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&CheckReport {
            port,
            in_use,
            owners: &owners,
        })?;
    } else if !in_use {
        renderer.line(format!("{port} is free"))?;
    } else {
        let names: Vec<String> = owners
            .iter()
            .map(|row| {
                format!(
                    "{} ({})",
                    row.process_name.clone().unwrap_or_else(unknown),
                    row.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into())
                )
            })
            .collect();
        renderer.line(format!("{port} is in use by {}", names.join(", ")))?;
    }

    // Probe semantics: success means "something holds this port", the way
    // `pgrep` and `nc -z` behave, so `if x port check 3000` reads naturally.
    Ok(if in_use { 0 } else { 1 })
}

/// Connection state counts, protocol split and kernel queue occupancy.
///
/// Counts are a pure reduction over the same snapshot every other command
/// renders, so the numbers always add up to what `x port all` would show.
pub fn stats(
    context: &SystemContext,
    renderer: &mut Renderer,
    options: &PortListOptions,
) -> Result<i32> {
    let rows = context.port.list(options)?;
    let stats = summarize(&rows);

    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&stats)?;
        return Ok(0);
    }

    let mut states = Table::new(["state", "sockets"]);
    for (state, count) in &stats.by_state {
        states.push(row![state_label(*state), count.to_string()]);
    }
    states.push(row!["total", stats.total.to_string()]);
    renderer.table(&states)?;

    let mut protocols = Table::new(["proto", "sockets"]);
    for (protocol, count) in &stats.by_protocol {
        protocols.push(row![protocol.clone(), count.to_string()]);
    }
    renderer.table(&protocols)?;

    if stats.queues.is_empty() {
        renderer.line("queue lengths: not reported by this platform")?;
    } else {
        let mut queues = Table::new(["queues", "value"]);
        if stats.queues.send_reporting > 0 {
            queues.push(row![
                format!("send queued ({} sockets)", stats.queues.send_reporting),
                format_bytes(stats.queues.send_bytes),
            ]);
        }
        if stats.queues.recv_reporting > 0 {
            queues.push(row![
                format!("recv queued ({} sockets)", stats.queues.recv_reporting),
                format_bytes(stats.queues.recv_bytes),
            ]);
        }
        queues.push(row![
            "backed up".to_string(),
            format!("{} sockets", stats.queues.backed_up),
        ]);
        renderer.table(&queues)?;
    }
    Ok(0)
}

/// Result of `x port check`.
#[derive(Debug, serde::Serialize)]
struct CheckReport<'a> {
    port: u16,
    in_use: bool,
    owners: &'a [PortInfo],
}

/// Show every socket held by processes whose name matches.
pub fn find(context: &SystemContext, renderer: &mut Renderer, name: &str) -> Result<i32> {
    let rows = context.port.find_process(name)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
    } else if rows.is_empty() {
        renderer.line(format!("no sockets for process `{name}`"))?;
    } else {
        let mut table = socket_table();
        for row in &rows {
            table.push(socket_row(row));
        }
        renderer.table(&table)?;
    }
    Ok(if rows.is_empty() { 3 } else { 0 })
}

/// One `x port watch` change event, as JSON.
#[derive(Debug, serde::Serialize)]
struct WatchEvent {
    time: String,
    added: Vec<PortInfo>,
    removed: Vec<PortInfo>,
}

/// Poll sockets and print only the differences between consecutive snapshots.
///
/// `--count n` bounds the loop so scripts and tests can take exactly `n`
/// samples; without it the command runs until Ctrl-C.
pub fn watch(
    context: &SystemContext,
    renderer: &mut Renderer,
    options: &PortListOptions,
    interval: f64,
    count: Option<usize>,
) -> Result<i32> {
    let offset = context
        .system
        .info()
        .ok()
        .and_then(|info| info.utc_offset_seconds)
        .unwrap_or(0);
    let interval = if interval.is_finite() {
        interval.max(0.05)
    } else {
        2.0
    };
    let json = renderer.format() == OutputFormat::Json;

    let mut previous: Option<Vec<PortInfo>> = None;
    let mut polls = 0usize;
    loop {
        let current = context.port.list(options)?;
        let time = x_core::format_timestamp(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            offset,
        );
        match &previous {
            None => {
                if !json {
                    renderer.line(format!(
                        "[{time}] watching {} socket(s), polling every {interval}s",
                        current.len()
                    ))?;
                }
            }
            Some(before) => {
                let diff = diff_sockets(before, &current);
                if !diff.is_empty() {
                    if json {
                        renderer.always_json(&WatchEvent {
                            time,
                            added: diff.added,
                            removed: diff.removed,
                        })?;
                    } else {
                        for row in &diff.removed {
                            renderer.line(format!("[{time}] - {}", watch_row(row)))?;
                        }
                        for row in &diff.added {
                            renderer.line(format!("[{time}] + {}", watch_row(row)))?;
                        }
                    }
                }
            }
        }
        renderer.flush().map_err(|e| {
            Error::new(
                x_core::ErrorKind::System,
                format!("watch output failed: {e}"),
            )
        })?;
        previous = Some(current);
        polls += 1;
        if count.is_some_and(|target| polls >= target) {
            return Ok(0);
        }
        std::thread::sleep(Duration::from_secs_f64(interval));
    }
}

/// One socket as a single watch-diff line.
fn watch_row(row: &PortInfo) -> String {
    format!(
        "{} {} pid {} {} {} -> {}",
        row.protocol.name(),
        state_label(row.state),
        row.pid.map(|p| p.to_string()).unwrap_or_else(unknown),
        row.process_name.clone().unwrap_or_else(unknown),
        row.endpoint(),
        remote_label(row),
    )
}

fn remote_label(row: &PortInfo) -> String {
    row.remote_socket_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|| "-".to_string())
}

/// Build the plan, show it, ask, then execute exactly that plan.
#[allow(clippy::too_many_arguments)]
fn kill(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    query: PortQuery,
    signal: x_core::KillSignal,
    assume_yes: bool,
) -> Result<i32> {
    let plan = context.port.plan(&query, PortSort::Port)?;
    let json = renderer.format() == OutputFormat::Json;
    if json {
        // One document per invocation: the outcome is appended to it below.
    } else if plan.is_empty() {
        renderer.line(format!("nothing matches {}", describe(&query)))?;
        return Ok(3);
    } else {
        let mut table = Table::new(["port", "proto", "state", "pid", "process", "user"]);
        for row in &plan.sockets {
            table.push(row![
                row.local_port.to_string(),
                row.protocol.name().to_string(),
                state_label(row.state),
                optional(row.pid),
                row.process_name.clone().unwrap_or_else(unknown),
                row.user.clone().unwrap_or_else(unknown),
            ]);
        }
        renderer.table(&table)?;

        let current = context.current_user();
        let elevated = plan.elevated_pids(current.as_deref());
        if !elevated.is_empty() {
            renderer.line(format!(
                "note: {} owned by other users, this needs root",
                join(&elevated)
            ))?;
        }
        renderer.line(format!(
            "plan: terminate {} process(es) to free {}",
            plan.target_pids().len(),
            describe(&query)
        ))?;
    }

    if !plan.is_satisfiable() {
        if json {
            renderer.always_json(&KillReport {
                plan: &plan,
                killed: None,
                aborted: false,
            })?;
        }
        return Ok(3);
    }
    if crate::dry_run_guard(
        renderer,
        &format!(
            "terminate {} process(es) to free {}",
            plan.target_pids().len(),
            describe(&query)
        ),
    )? {
        return Ok(0);
    }
    if !assume_yes {
        let question = format!("kill {} process(es)?", plan.target_pids().len());
        if !confirm_or_fail(confirmer, &question)? {
            if json {
                renderer.always_json(&KillReport {
                    plan: &plan,
                    killed: None,
                    aborted: true,
                })?;
            } else {
                renderer.line("aborted")?;
            }
            return Ok(super::EXIT_DECLINED);
        }
    }

    let killed = context.port.kill_plan(&plan, signal)?;
    if json {
        renderer.always_json(&KillReport {
            plan: &plan,
            killed: Some(killed),
            aborted: false,
        })?;
    } else {
        renderer.line(format!("killed {killed} process(es)"))?;
    }
    Ok(0)
}

/// Result of `x port kill`, as a single JSON document.
#[derive(Debug, serde::Serialize)]
struct KillReport<'a> {
    plan: &'a KillPlan,
    killed: Option<usize>,
    aborted: bool,
}

/// Ask for confirmation. `false` means "declined", never "unknown".
pub(crate) fn confirm_or_fail(confirmer: &mut dyn Confirmer, question: &str) -> Result<bool> {
    confirmer.confirm(question).map_err(|e| {
        Error::new(
            x_core::ErrorKind::System,
            format!("cannot read confirmation: {e}"),
        )
    })
}

fn socket_table() -> Table {
    Table::new([
        "port", "proto", "state", "pid", "process", "user", "local", "remote",
    ])
}

fn socket_row(row: &x_core::port::PortInfo) -> Vec<crate::format::Cell> {
    row![
        row.local_port.to_string(),
        row.protocol.name().to_string(),
        state_label(row.state),
        optional(row.pid),
        row.process_name.clone().unwrap_or_else(unknown),
        row.user.clone().unwrap_or_else(unknown),
        row.endpoint(),
        remote_label(row),
    ]
}

fn state_label(state: ConnectionState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

fn unknown() -> String {
    "-".to_string()
}

fn optional(value: Option<u32>) -> String {
    value.map(|v| v.to_string()).unwrap_or_else(unknown)
}

fn join(values: &[u32]) -> String {
    values
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn describe(query: &PortQuery) -> String {
    match query {
        PortQuery::Port(port) => format!("port {port}"),
        PortQuery::Process(name) => format!("process `{name}`"),
    }
}

/// `x port <n>`: owner, CWD, git repository, project kind and start command.
///
/// The chain is port → process → working directory → project detection; every
/// hop degrades honestly when the platform cannot see it.
pub fn who(context: &SystemContext, renderer: &mut Renderer, port: u16) -> Result<i32> {
    let owners = context.port.find_port(port)?;
    if renderer.format() == OutputFormat::Json {
        let reports: Vec<PortWhoReport> = owners
            .iter()
            .map(|owner| port_who_report(context, port, owner))
            .collect();
        renderer.always_json(&reports)?;
        return Ok(0);
    }
    if owners.is_empty() {
        renderer.line(format!("{port} is free"))?;
        return Ok(1);
    }
    for owner in &owners {
        let report = port_who_report(context, port, owner);
        renderer.line(format!(
            "{port}: {} ({})",
            owner.process_name.clone().unwrap_or_else(unknown),
            owner.pid.map(|p| p.to_string()).unwrap_or_else(unknown),
        ))?;
        if let Some(cwd) = &report.cwd {
            renderer.line(format!("cwd: {cwd}"))?;
        }
        if let Some(git_root) = &report.git_root {
            renderer.line(format!("git: {git_root}"))?;
        }
        if let Some(branch) = &report.branch {
            renderer.line(format!("branch: {branch}"))?;
        }
        if let Some(project) = &report.project {
            renderer.line(format!("project: {project}"))?;
        }
        if let Some(manager) = &report.package_manager {
            renderer.line(format!("package manager: {manager}"))?;
        }
        if let Some(start) = &report.start_command {
            renderer.line(format!("start: {start}"))?;
        }
        if let Some(command_line) = &report.command_line {
            renderer.line(format!("command: {command_line}"))?;
        }
    }
    Ok(0)
}

/// What `x port <n>` reports for one owning process.
#[derive(Debug, serde::Serialize)]
struct PortWhoReport {
    port: u16,
    pid: Option<u32>,
    process: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_manager: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command_line: Option<String>,
}

fn port_who_report(context: &SystemContext, port: u16, socket: &PortInfo) -> PortWhoReport {
    let pid = socket.pid;
    let process = socket.process_name.clone().unwrap_or_else(unknown);

    let detail = pid.and_then(|pid| context.process.get(pid).ok());
    let cwd = detail.as_ref().and_then(|p| p.cwd.clone());
    let command_line = detail.as_ref().and_then(|p| p.command_line.clone());

    let mut report = PortWhoReport {
        port,
        pid,
        process,
        cwd,
        git_root: None,
        branch: None,
        project: None,
        package_manager: None,
        start_command: None,
        command_line,
    };

    if let Some(cwd) = &report.cwd {
        let path = std::path::Path::new(cwd);
        report.git_root = x_core::gitcmd::repo_root(path)
            .ok()
            .map(|root| root.display().to_string());
        let project = x_core::project::detect(path);
        report.branch = project.branch.clone();
        report.package_manager = project.package_manager.map(|m| m.name().to_string());
        report.start_command = project
            .package_manager
            .and_then(|m| m.command())
            .map(str::to_string);
        if !project.kinds.is_empty() {
            report.project = Some(project.kinds.join(", "));
        }
    }
    report
}
