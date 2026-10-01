//! `x shell`: which shell am I in, which exist, which is the default.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x shell` subcommands.
#[derive(Debug, Subcommand)]
pub enum ShellCommand {
    /// The shell this process runs under.
    Info,

    /// Shells installed on this machine.
    List,

    /// The shell new login sessions get.
    Default,
}

/// Arguments for `x shell`.
#[derive(Debug, clap::Args)]
pub struct ShellArgs {
    #[command(subcommand)]
    pub command: ShellCommand,
}

/// Route a `x shell` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &ShellCommand,
) -> Result<i32> {
    let shell = context
        .shell
        .as_ref()
        .ok_or_else(|| Error::unsupported("shell detection is not available in this context"))?;

    let render_one = |renderer: &mut Renderer, info: &x_core::shell::ShellInfo| -> Result<()> {
        if renderer.format() == OutputFormat::Json {
            renderer.always_json(info)?;
        } else {
            let mut table = Table::new(["field", "value"]);
            table.push(row!["name", info.name.clone()]);
            table.push(row![
                "path",
                info.path.clone().unwrap_or_else(|| "-".into()),
            ]);
            table.push(row![
                "version",
                info.version.clone().unwrap_or_else(|| "-".into()),
            ]);
            renderer.table(&table)?;
        }
        Ok(())
    };

    match command {
        ShellCommand::Info => {
            render_one(renderer, &shell.current()?)?;
        }
        ShellCommand::Default => {
            render_one(renderer, &shell.default()?)?;
        }
        ShellCommand::List => {
            let shells = shell.list()?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&shells)?;
                return Ok(0);
            }
            let mut table = Table::new(["name", "path", "version"]);
            for info in &shells {
                table.push(row![
                    info.name.clone(),
                    info.path.clone().unwrap_or_else(|| "-".into()),
                    info.version.clone().unwrap_or_else(|| "-".into()),
                ]);
            }
            renderer.table(&table)?;
        }
    }
    Ok(0)
}
