//! `x startup`: login items across the platform's startup stores.
//!
//! `enable` / `disable` change the store (registry, `.desktop` files, plists)
//! and are confirmed; the honest limits of each store (e.g. Windows Run
//! entries cannot be restored after deletion) surface as Unsupported errors.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{Confirmer, OutputFormat, Renderer, Table},
    row,
};

/// `x startup` subcommands.
#[derive(Debug, Subcommand)]
pub enum StartupCommand {
    /// Everything that would start at login.
    List,

    /// Enable a disabled item (where the store allows it).
    Enable {
        /// Item name as shown by `list`.
        name: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Disable an item without deleting it where possible.
    Disable {
        /// Item name as shown by `list`.
        name: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// Arguments for `x startup`.
#[derive(Debug, clap::Args)]
pub struct StartupArgs {
    #[command(subcommand)]
    pub command: StartupCommand,
}

/// Route a `x startup` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    command: &StartupCommand,
) -> Result<i32> {
    let startup = context
        .startup
        .as_ref()
        .ok_or_else(|| Error::unsupported("startup management is not available in this context"))?;

    match command {
        StartupCommand::List => {
            let items = startup.list()?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&items)?;
                return Ok(0);
            }
            let mut table = Table::new(["name", "source", "enabled", "command"]);
            for item in &items {
                table.push(row![
                    item.name.clone(),
                    item.source.name().to_string(),
                    if item.enabled { "✓" } else { "✗" }.to_string(),
                    item.command.clone().unwrap_or_else(|| "-".into()),
                ]);
            }
            renderer.table(&table)?;
        }
        StartupCommand::Enable { name, yes } => {
            confirm(renderer, confirmer, *yes, &format!("enable {name}"))?;
            if crate::dry_run_guard(renderer, &format!("enable startup item {name}"))? {
                return Ok(0);
            }
            startup.enable(name)?;
            renderer.line(format!("enabled {name}"))?;
        }
        StartupCommand::Disable { name, yes } => {
            confirm(renderer, confirmer, *yes, &format!("disable {name}"))?;
            if crate::dry_run_guard(renderer, &format!("disable startup item {name}"))? {
                return Ok(0);
            }
            startup.disable(name)?;
            renderer.line(format!("disabled {name}"))?;
        }
    }
    Ok(0)
}

fn confirm(
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    yes: bool,
    action: &str,
) -> Result<()> {
    if yes || renderer.format() == OutputFormat::Json {
        return Ok(());
    }
    renderer.line(format!("about to {action}"))?;
    if confirmer.confirm("continue?")? {
        Ok(())
    } else {
        Err(Error::invalid_input("aborted by user"))
    }
}
