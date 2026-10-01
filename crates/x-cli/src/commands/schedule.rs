//! `x schedule`: scheduled tasks through the platform's own store.
//!
//! `add` / `remove` write crontab / schtasks / launchd entries and are
//! confirmed; each adapter documents the schedule grammar it accepts.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{Confirmer, OutputFormat, Renderer, Table},
    row,
};

/// `x schedule` subcommands.
#[derive(Debug, Subcommand)]
pub enum ScheduleCommand {
    /// Everything scheduled (crontab lines / user tasks / launchd jobs).
    List,

    /// Create or overwrite a task.
    Add {
        /// Task name (crontab lines are addressed as `crontab:<n>`).
        name: String,
        /// Schedule: cron syntax (Linux), `daily|hourly|onlogon|every=N`
        /// (Windows), a number of minutes (macOS).
        schedule: String,
        /// Command the task runs.
        command: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Remove a task by name.
    Remove {
        /// Task name as shown by `list`.
        name: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// Arguments for `x schedule`.
#[derive(Debug, clap::Args)]
pub struct ScheduleArgs {
    #[command(subcommand)]
    pub command: ScheduleCommand,
}

/// Route a `x schedule` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    command: &ScheduleCommand,
) -> Result<i32> {
    let schedule = context.schedule.as_ref().ok_or_else(|| {
        Error::unsupported("schedule management is not available in this context")
    })?;

    match command {
        ScheduleCommand::List => {
            let entries = schedule.list()?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&entries)?;
                return Ok(0);
            }
            let mut table = Table::new(["name", "enabled", "schedule", "command"]);
            for entry in &entries {
                table.push(row![
                    entry.name.clone(),
                    if entry.enabled { "✓" } else { "✗" }.to_string(),
                    entry
                        .trigger
                        .as_ref()
                        .map(|t| t.spec.clone())
                        .unwrap_or_else(|| "-".into()),
                    entry.command.clone(),
                ]);
            }
            renderer.table(&table)?;
        }
        ScheduleCommand::Add {
            name,
            schedule: spec,
            command: job,
            yes,
        } => {
            if !(*yes || renderer.format() == OutputFormat::Json) {
                renderer.line(format!("about to schedule `{job}` as {name} ({spec})"))?;
                if !confirmer.confirm("continue?")? {
                    return Err(Error::invalid_input("aborted by user"));
                }
            }
            schedule.add(name, spec, job)?;
            renderer.line(format!("scheduled {name}"))?;
        }
        ScheduleCommand::Remove { name, yes } => {
            if !(*yes || renderer.format() == OutputFormat::Json) {
                renderer.line(format!("about to remove scheduled task {name}"))?;
                if !confirmer.confirm("continue?")? {
                    return Err(Error::invalid_input("aborted by user"));
                }
            }
            schedule.remove(name)?;
            renderer.line(format!("removed {name}"))?;
        }
    }
    Ok(0)
}
