//! `x port`: sockets, owners and the kill plan.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::port::{
    ConnectionState, KillPlan, PortInfo, PortListOptions, PortOwner, PortQuery, PortSort, Protocol,
};
use x_core::SystemContext;

use crate::SignalArg;
use crate::{
    format::{Confirmer, OutputFormat, Renderer, Table},
    row,
};

/// `x port`: filters apply to the implicit `list`, subcommands do the rest.
#[derive(Debug, clap::Args)]
pub struct PortArgs {
    /// Subcommand. Omitted means "list listening sockets".
    #[command(subcommand)]
    pub command: Option<PortCommand>,
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
        return list(context, renderer, &args.list.options(true));
    };
    match command {
        PortCommand::List(args) => list(context, renderer, &args.options(true)),
        PortCommand::All(args) => list(context, renderer, &args.options(false)),
        PortCommand::Owners(args) => owners(context, renderer, &args.options(true)),
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

/// Result of `x port check`.
#[derive(Debug, serde::Serialize)]
struct CheckReport<'a> {
    port: u16,
    in_use: bool,
    owners: &'a [PortInfo],
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
            return Ok(130);
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
    Table::new(["port", "proto", "state", "pid", "process", "user", "local"])
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
