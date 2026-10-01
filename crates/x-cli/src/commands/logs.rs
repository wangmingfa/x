//! `x logs`: read the machine's own log store.
//!
//! Pure reads — no confirmation, no elevation, no audit line. The three
//! platform stores differ in what they can scope by; the adapter reports
//! honestly (exit 7) where its store has no such index.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::logs::LogScope;
use x_core::SystemContext;

use crate::format::{OutputFormat, Renderer, Table};
use crate::row;

/// Default rows fetched when `--limit` is not given.
const DEFAULT_LIMIT: usize = 50;

/// `x logs` subcommands.
#[derive(Debug, Subcommand)]
pub enum LogsCommand {
    /// The machine-wide log: journal, System channel, unified log.
    System {
        /// Maximum number of rows (newest first).
        #[arg(long)]
        limit: Option<usize>,
    },

    /// Records attributed to one service (unit / event provider / daemon).
    Service {
        /// Service name, e.g. `nginx` or `Service Control Manager`.
        name: String,
        /// Maximum number of rows (newest first).
        #[arg(long)]
        limit: Option<usize>,
    },

    /// Records attributed to one process, by pid or program name.
    Process {
        /// Process id or program name.
        target: String,
        /// Maximum number of rows (newest first).
        #[arg(long)]
        limit: Option<usize>,
    },
}

/// Arguments for `x logs`.
#[derive(Debug, clap::Args)]
pub struct LogsArgs {
    #[command(subcommand)]
    pub command: LogsCommand,
}

/// Route a `x logs` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &LogsCommand,
) -> Result<i32> {
    let reader = context
        .logs
        .as_ref()
        .ok_or_else(|| Error::unsupported("no system log reader in this context"))?;

    let (scope, limit) = match command {
        LogsCommand::System { limit } => (LogScope::System, limit),
        LogsCommand::Service { name, limit } => (LogScope::Service(name.clone()), limit),
        LogsCommand::Process { target, limit } => (LogScope::Process(target.clone()), limit),
    };
    let page = reader.read(&scope, limit.unwrap_or(DEFAULT_LIMIT))?;

    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&page)?;
        return Ok(0);
    }

    if page.entries.is_empty() {
        renderer.line(format!(
            "no records for {} from {}",
            page.scope, page.source
        ))?;
        return Ok(0);
    }

    let mut table = Table::new(["timestamp", "level", "origin", "message"]);
    for entry in &page.entries {
        table.push(row![
            entry.timestamp.clone().unwrap_or_else(|| "-".into()),
            entry.level.clone().unwrap_or_else(|| "-".into()),
            entry.origin.clone().unwrap_or_else(|| "-".into()),
            flatten(&entry.message),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// Event records carry multi-paragraph messages; a table row stays on one line.
fn flatten(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}
