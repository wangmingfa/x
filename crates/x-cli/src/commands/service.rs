//! `x service`: list, inspect and drive the platform service manager.

use clap::Subcommand;
use x_core::error::Result;
use x_core::service::{ServiceAction, ServiceInfo, ServiceListOptions, ServiceState};
use x_core::SystemContext;

use crate::{
    format::{Confirmer, OutputFormat, Renderer, Table},
    row,
};

/// `x service` subcommands.
#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    /// List services.
    List {
        /// Case insensitive name filter.
        #[arg(long)]
        search: Option<String>,
        /// Only running services.
        #[arg(long)]
        running: bool,
        /// Maximum number of rows.
        #[arg(long)]
        limit: Option<usize>,
    },

    /// Detail for one service.
    Status {
        /// Service name, e.g. `sshd`, `com.apple.something`, `Spooler`.
        name: String,
    },

    /// Start a service.
    Start(ActionArgs),

    /// Stop a service.
    Stop(ActionArgs),

    /// Stop then start.
    Restart(ActionArgs),

    /// Reload configuration.
    Reload(ActionArgs),

    /// Enable at boot.
    Enable(ActionArgs),

    /// Disable at boot.
    Disable(ActionArgs),
}

/// Arguments shared by every action.
#[derive(Debug, clap::Args)]
pub struct ActionArgs {
    /// Service name.
    pub name: String,
    /// Do not ask for confirmation.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

impl ActionArgs {
    /// Whether `--yes` was given.
    pub fn assume_yes(&self) -> bool {
        self.yes
    }
}

/// Whether the action can interrupt running work, and therefore needs a
/// confirmation. Starting a service that is already stopped is harmless.
fn is_disruptive(action: ServiceAction) -> bool {
    matches!(
        action,
        ServiceAction::Stop | ServiceAction::Restart | ServiceAction::Disable
    )
}

/// Route a `x service` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    command: &ServiceCommand,
) -> Result<i32> {
    match command {
        ServiceCommand::List {
            search,
            running,
            limit,
        } => list(
            context,
            renderer,
            &ServiceListOptions {
                search: search.clone(),
                state: None,
                running_only: *running,
                limit: *limit,
            },
        ),
        ServiceCommand::Status { name } => status(context, renderer, name),
        ServiceCommand::Start(args) => {
            apply(context, renderer, confirmer, args, ServiceAction::Start)
        }
        ServiceCommand::Stop(args) => {
            apply(context, renderer, confirmer, args, ServiceAction::Stop)
        }
        ServiceCommand::Restart(args) => {
            apply(context, renderer, confirmer, args, ServiceAction::Restart)
        }
        ServiceCommand::Reload(args) => {
            apply(context, renderer, confirmer, args, ServiceAction::Reload)
        }
        ServiceCommand::Enable(args) => {
            apply(context, renderer, confirmer, args, ServiceAction::Enable)
        }
        ServiceCommand::Disable(args) => {
            apply(context, renderer, confirmer, args, ServiceAction::Disable)
        }
    }
}

/// List services.
pub fn list(
    context: &SystemContext,
    renderer: &mut Renderer,
    options: &ServiceListOptions,
) -> Result<i32> {
    let rows = context.service.list(options)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(0);
    }
    let mut table = service_table();
    for row in &rows {
        table.push(service_row(row));
    }
    renderer.table(&table)?;
    Ok(0)
}

/// Detail for one service.
pub fn status(context: &SystemContext, renderer: &mut Renderer, name: &str) -> Result<i32> {
    let row = context.service.status(name)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&row)?;
        return Ok(0);
    }
    let mut table = Table::new(["field", "value"]);
    table.push(row!["name", row.name.clone()]);
    if let Some(display) = &row.display_name {
        table.push(row!["display name", display.clone()]);
    }
    if let Some(description) = &row.description {
        table.push(row!["description", description.clone()]);
    }
    table.push(row!["state", state_label(row.state)]);
    table.push(row![
        "pid",
        row.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
    ]);
    table.push(row![
        "enabled",
        match row.enabled {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        }
        .to_string(),
    ]);
    table.push(row!["manager", manager_label(row.manager)]);
    renderer.table(&table)?;
    Ok(0)
}

/// Apply a lifecycle action, confirming disruptive ones.
fn apply(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    args: &ActionArgs,
    action: ServiceAction,
) -> Result<i32> {
    let name = args.name.as_str();
    let before = context.service.status(name)?;

    if is_disruptive(action) && !args.assume_yes() {
        let question = format!("{} service `{name}`?", verb(action));
        if !super::port::confirm_or_fail(confirmer, &question)? {
            renderer.line("aborted")?;
            return Ok(130);
        }
    }

    context.service.action(name, action)?;
    let after = context.service.status(name).unwrap_or(before);
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&after)?;
    } else {
        let mut table = service_table();
        table.push(service_row(&after));
        renderer.table(&table)?;
    }
    Ok(0)
}

fn service_table() -> Table {
    Table::new(["name", "state", "pid", "enabled", "manager", "display name"])
}

fn service_row(row: &ServiceInfo) -> Vec<crate::format::Cell> {
    row![
        row.name.clone(),
        state_label(row.state),
        row.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
        match row.enabled {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        }
        .to_string(),
        manager_label(row.manager),
        row.display_name.clone().unwrap_or_default(),
    ]
}

fn state_label(state: ServiceState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

fn manager_label(manager: x_core::service::ServiceManagerType) -> String {
    serde_json::to_value(manager)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

fn verb(action: ServiceAction) -> &'static str {
    match action {
        ServiceAction::Start => "start",
        ServiceAction::Stop => "stop",
        ServiceAction::Restart => "restart",
        ServiceAction::Reload => "reload",
        ServiceAction::Enable => "enable",
        ServiceAction::Disable => "disable",
    }
}
