//! `x ps`: process listing, detail, tree and termination.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::process::{diff_processes, ProcessInfo, ProcessListOptions, ProcessNode, ProcessSort};
use x_core::KillSignal;
use x_core::SystemContext;

use crate::format::{clip, Confirmer, OutputFormat, Renderer, Table};
use crate::{row, SignalArg};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Width of the command column in the human format. `--json` keeps it whole.
const COMMAND_WIDTH: usize = 56;

/// `x ps`: an optional search term plus flags, with subcommands for the rest.
#[derive(Debug, clap::Args)]
pub struct PsArgs {
    /// Subcommand. Omitted means "list processes".
    #[command(subcommand)]
    pub command: Option<PsCommand>,
    /// Listing filters, used without a subcommand.
    #[command(flatten)]
    pub list: PsListArgs,
}

/// `x ps` subcommands.
#[derive(Debug, Subcommand)]
pub enum PsCommand {
    /// List processes, busiest first. Accepts an optional search term.
    List(PsListArgs),

    /// Show the process tree instead of a flat list.
    Tree {
        /// Listing filters.
        #[command(flatten)]
        args: PsListArgs,

        /// Collapse processes that have children into a single row.
        #[arg(long, short = 'c')]
        collapsed: bool,
    },

    /// Detail for one pid.
    Show {
        /// Process id.
        pid: u32,
    },

    /// Terminate one process by pid.
    Kill {
        /// Process id.
        pid: u32,
        /// Signal to deliver.
        #[arg(long, value_enum, default_value_t = SignalArg::Term)]
        signal: SignalArg,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Terminate every process whose name matches.
    KillByName {
        /// Process name substring.
        name: String,
        /// Signal to deliver.
        #[arg(long, value_enum, default_value_t = SignalArg::Term)]
        signal: SignalArg,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Poll the process table and print only arrivals and departures.
    Watch {
        /// Listing filters.
        #[command(flatten)]
        args: PsListArgs,

        /// Seconds between samples.
        #[arg(long, default_value_t = 2.0)]
        interval: f64,

        /// Stop after this many samples; without it, run until Ctrl-C.
        #[arg(long)]
        count: Option<usize>,
    },
}

/// Listing filters.
#[derive(Debug, Default, clap::Args)]
pub struct PsListArgs {
    /// Case insensitive substring of name, command line or executable.
    pub search: Option<String>,
    /// Only processes owned by this user.
    #[arg(long)]
    pub user: Option<String>,
    /// Sort key.
    #[arg(long, value_enum, default_value_t = SortArg::Cpu)]
    pub sort: SortArg,
    /// Maximum number of rows.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Skip CPU and memory accounting for a cheaper snapshot.
    #[arg(long)]
    pub light: bool,
}

/// Sort key values.
#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
pub enum SortArg {
    /// Highest CPU usage first.
    #[default]
    Cpu,
    /// Highest resident memory first.
    Memory,
    /// Process id.
    Pid,
    /// Process name.
    Name,
    /// Oldest process first.
    StartTime,
}

impl From<SortArg> for ProcessSort {
    fn from(value: SortArg) -> Self {
        match value {
            SortArg::Cpu => ProcessSort::Cpu,
            SortArg::Memory => ProcessSort::Memory,
            SortArg::Pid => ProcessSort::Pid,
            SortArg::Name => ProcessSort::Name,
            SortArg::StartTime => ProcessSort::StartTime,
        }
    }
}

impl PsListArgs {
    fn options(&self) -> ProcessListOptions {
        ProcessListOptions {
            search: self.search.clone(),
            user: self.user.clone(),
            sort: self.sort.into(),
            limit: self.limit,
            with_usage: !self.light,
        }
    }
}

/// Route a `x ps` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    args: &PsArgs,
) -> Result<i32> {
    let Some(command) = &args.command else {
        return list(context, renderer, &args.list.options());
    };
    match command {
        PsCommand::List(args) => list(context, renderer, &args.options()),
        PsCommand::Tree { args, collapsed } => tree(context, renderer, &args.options(), *collapsed),
        PsCommand::Show { pid } => show(context, renderer, *pid),
        PsCommand::Kill { pid, signal, yes } => kill(
            context,
            renderer,
            confirmer,
            vec![*pid],
            (*signal).into(),
            *yes,
        ),
        PsCommand::KillByName { name, signal, yes } => {
            let pids: Vec<u32> = context
                .process
                .find(name)?
                .into_iter()
                .map(|p| p.pid)
                .collect();
            kill(context, renderer, confirmer, pids, (*signal).into(), *yes)
        }
        PsCommand::Watch {
            args,
            interval,
            count,
        } => watch(context, renderer, &args.options(), *interval, *count),
    }
}

/// One `x ps watch` change event, as JSON.
#[derive(Debug, serde::Serialize)]
struct WatchEvent {
    time: String,
    added: Vec<ProcessInfo>,
    removed: Vec<ProcessInfo>,
}

/// Poll the process table and print only what changed since the previous
/// sample.
///
/// Same shape as `x port watch`: the first poll prints a baseline line, later
/// polls print only arrivals (`+`) and departures (`-`), and `--count n` bounds
/// the loop so scripts and tests can take exactly `n` samples. A snapshot that
/// fails to be sampled does not end the watch: the error is reported and the
/// previous baseline is kept, because a transient permission failure should
/// not look like "every process exited".
pub fn watch(
    context: &SystemContext,
    renderer: &mut Renderer,
    options: &ProcessListOptions,
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

    let mut previous: Option<Vec<ProcessInfo>> = None;
    let mut polls = 0usize;
    loop {
        let current = match context.process.list(options) {
            Ok(rows) => rows,
            Err(err) => {
                if json {
                    renderer.always_json(&serde_json::json!({
                        "time": stamp(offset),
                        "error": err.to_string(),
                    }))?;
                } else {
                    renderer.line(format!("[{}] sample failed: {err}", stamp(offset)))?;
                }
                renderer.flush().map_err(|e| {
                    Error::new(
                        x_core::ErrorKind::System,
                        format!("watch output failed: {e}"),
                    )
                })?;
                std::thread::sleep(Duration::from_secs_f64(interval));
                polls += 1;
                if count.is_some_and(|target| polls >= target) {
                    return Ok(0);
                }
                continue;
            }
        };

        match &previous {
            None => {
                if !json {
                    renderer.line(format!(
                        "[{}] watching {} process(es), polling every {interval}s",
                        stamp(offset),
                        current.len()
                    ))?;
                }
            }
            Some(before) => {
                let diff = diff_processes(before, &current);
                if !diff.is_empty() {
                    if json {
                        renderer.always_json(&WatchEvent {
                            time: stamp(offset),
                            added: diff.added,
                            removed: diff.removed,
                        })?;
                    } else {
                        for row in &diff.removed {
                            renderer.line(format!("[{}] - {}", stamp(offset), watch_row(row)))?;
                        }
                        for row in &diff.added {
                            renderer.line(format!("[{}] + {}", stamp(offset), watch_row(row)))?;
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

/// Current wall-clock stamp in the machine's own UTC offset.
fn stamp(offset: i64) -> String {
    x_core::format_timestamp(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        offset,
    )
}

/// One process as a single watch-diff line.
fn watch_row(row: &ProcessInfo) -> String {
    format!(
        "pid {} {} {}",
        row.pid,
        row.state.label(),
        clip(
            row.command_line.as_deref().unwrap_or(row.name.as_str()),
            COMMAND_WIDTH
        )
    )
}

/// Flat process list.
pub fn list(
    context: &SystemContext,
    renderer: &mut Renderer,
    options: &ProcessListOptions,
) -> Result<i32> {
    let rows = context.process.list(options)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
    } else {
        let mut table = Table::new(["pid", "user", "cpu%", "mem", "name", "command"]);
        for row in &rows {
            table.push(process_row(row));
        }
        renderer.table(&table)?;
    }
    Ok(0)
}

/// Parent/child tree, useful to see what a daemon spawned.
pub fn tree(
    context: &SystemContext,
    renderer: &mut Renderer,
    options: &ProcessListOptions,
    collapsed: bool,
) -> Result<i32> {
    let tree = context.process.tree(options)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&tree)?;
        return Ok(0);
    }

    let mut table = Table::new(["pid", "ppid", "cpu%", "mem", "name"]);
    for node in &tree.roots {
        if collapsed {
            walk_collapsed(node, 0, &mut table);
        } else {
            walk(node, 0, &mut table);
        }
    }
    renderer.table(&table)?;
    Ok(0)
}

fn walk(node: &ProcessNode, depth: usize, table: &mut Table) {
    let indent = "  ".repeat(depth);
    table.push(row![
        node.process.pid.to_string(),
        node.process
            .parent_pid
            .map(|p| p.to_string())
            .unwrap_or_default(),
        format!("{indent}{}", cpu(&node.process)),
        memory(&node.process),
        node.process.name.clone(),
    ]);
    for child in &node.children {
        walk(child, depth + 1, table);
    }
}

/// Collapsed tree: a node with children becomes one row that names the
/// hidden subtree size, so a deep daemon chain fits on one screen.
fn walk_collapsed(node: &ProcessNode, depth: usize, table: &mut Table) {
    let indent = "  ".repeat(depth);
    let name = if node.children.is_empty() {
        node.process.name.clone()
    } else {
        format!("{} (+{} hidden)", node.process.name, node.depth_total() - 1)
    };
    table.push(row![
        node.process.pid.to_string(),
        node.process
            .parent_pid
            .map(|p| p.to_string())
            .unwrap_or_default(),
        format!("{indent}{}", cpu(&node.process)),
        memory(&node.process),
        name,
    ]);
}

/// Detail for a single process.
pub fn show(context: &SystemContext, renderer: &mut Renderer, pid: u32) -> Result<i32> {
    let process = context.process.get(pid)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&process)?;
        return Ok(0);
    }
    let mut table = Table::new(["field", "value"]);
    table.push(row!["pid", process.pid.to_string()]);
    if let Some(parent) = process.parent_pid {
        table.push(row!["ppid", parent.to_string()]);
    }
    table.push(row!["name", process.name.clone()]);
    if let Some(exe) = &process.executable {
        table.push(row!["executable", exe.clone()]);
    }
    if let Some(user) = &process.user {
        table.push(row!["user", user.clone()]);
    }
    if let Some(command) = &process.command_line {
        table.push(row!["command", command.clone()]);
    }
    if let Some(threads) = process.threads {
        table.push(row!["threads", threads.to_string()]);
    }
    if let Some(cpu) = process.cpu_usage {
        table.push(row!["cpu%", format!("{cpu:.1}")]);
    }
    if let Some(mem) = process.memory_bytes {
        table.push(row!["memory", x_core::format_bytes(mem)]);
    }
    if let Some(cwd) = &process.cwd {
        table.push(row!["cwd", cwd.clone()]);
    }
    if let Some(connections) = &process.connections {
        if !connections.is_empty() {
            let mut table = Table::new(["proto", "local", "remote"]);
            for connection in connections {
                table.push(row![
                    connection.protocol.clone(),
                    connection.local.clone(),
                    connection.remote.clone(),
                ]);
            }
            renderer.line("connections")?;
            renderer.table(&table)?;
        }
    }
    if let Some(files) = &process.open_files {
        if !files.is_empty() {
            renderer.line(format!("open files ({})", files.len()))?;
            for file in files {
                renderer.line(format!("  {file}"))?;
            }
        }
    }
    if let Some(environment) = &process.environment {
        if !environment.is_empty() {
            renderer.line("environment")?;
            for (key, value) in environment {
                renderer.line(format!("  {key}={value}"))?;
            }
        }
    }
    renderer.table(&table)?;
    Ok(0)
}

/// Terminate the given pids after showing what was selected.
fn kill(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    pids: Vec<u32>,
    signal: KillSignal,
    assume_yes: bool,
) -> Result<i32> {
    let json = renderer.format() == OutputFormat::Json;

    if pids.is_empty() {
        let victims: Vec<ProcessInfo> = Vec::new();
        if json {
            renderer.always_json(&KillReport {
                victims: &victims,
                killed: 0,
                failures: &[],
                aborted: false,
            })?;
        } else {
            renderer.line("no matching process")?;
        }
        return Ok(3);
    }

    let victims: Vec<ProcessInfo> = pids
        .iter()
        .filter_map(|pid| context.process.get(*pid).ok())
        .collect();

    if !json {
        let mut table = Table::new(["pid", "user", "cpu%", "mem", "name", "command"]);
        for row in &victims {
            table.push(process_row(row));
        }
        renderer.table(&table)?;
    }

    if crate::dry_run_guard(
        renderer,
        &format!("terminate {} process(es) with {:?}", pids.len(), signal),
    )? {
        return Ok(0);
    }

    if !assume_yes {
        let question = format!("kill {} process(es)?", victims.len());
        if !super::port::confirm_or_fail(confirmer, &question)? {
            if json {
                renderer.always_json(&KillReport {
                    victims: &victims,
                    killed: 0,
                    failures: &[],
                    aborted: true,
                })?;
            } else {
                renderer.line("aborted")?;
            }
            return Ok(super::EXIT_DECLINED);
        }
    }

    let failures = context.process.kill_many(&pids, signal)?;
    let killed = pids.len() - failures.len();
    let messages: Vec<String> = failures.iter().map(|f| f.message().to_string()).collect();

    if json {
        renderer.always_json(&KillReport {
            victims: &victims,
            killed,
            failures: &messages,
            aborted: false,
        })?;
    } else {
        renderer.line(format!("killed {killed} of {} process(es)", pids.len()))?;
        for message in &messages {
            renderer.line(format!("failed: {message}"))?;
        }
    }
    Ok(if failures.is_empty() { 0 } else { 1 })
}

/// Result of `x ps kill`, as a single JSON document.
#[derive(Debug, serde::Serialize)]
struct KillReport<'a> {
    victims: &'a [ProcessInfo],
    killed: usize,
    failures: &'a [String],
    aborted: bool,
}

fn process_row(row: &ProcessInfo) -> Vec<crate::format::Cell> {
    row![
        row.pid.to_string(),
        row.user.clone().unwrap_or_else(|| "-".into()),
        cpu(row),
        memory(row),
        row.name.clone(),
        row.command_line
            .as_deref()
            .map(|cmd| clip(cmd, COMMAND_WIDTH))
            .unwrap_or_default(),
    ]
}

fn cpu(row: &ProcessInfo) -> String {
    match row.cpu_usage {
        Some(value) if row.cpu_usage.is_some() => format!("{value:.1}"),
        _ => "-".into(),
    }
}

fn memory(row: &ProcessInfo) -> String {
    row.memory_bytes
        .map(x_core::format_bytes)
        .unwrap_or_default()
}
