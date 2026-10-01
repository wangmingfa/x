//! `x plugins`: manage external `x-<name>` plugins.
//!
//! Listing and installing only — running a plugin happens by typing `x <name>`
//! like any built-in subcommand, which keeps the plugin surface identical for
//! scripts and humans.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{Confirmer, OutputFormat, Renderer, Table},
    row,
};

/// `x plugins` subcommands.
#[derive(Debug, Subcommand)]
pub enum PluginsCommand {
    /// List installed plugins.
    List,

    /// Install (or replace) a plugin from a binary path.
    Install {
        /// Path to the plugin executable.
        source: String,
        /// Name to install as (invoked as `x <name>`); defaults to the file stem.
        #[arg(long)]
        name: Option<String>,
    },

    /// Remove an installed plugin.
    Remove {
        /// Plugin name as shown by `list`.
        name: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// Arguments for `x plugins`.
#[derive(Debug, clap::Args)]
pub struct PluginsArgs {
    #[command(subcommand)]
    pub command: PluginsCommand,
}

/// Route a `x plugins` invocation.
pub fn dispatch(
    _context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    command: &PluginsCommand,
) -> Result<i32> {
    match command {
        PluginsCommand::List => {
            let plugins = x_core::plugins::list();
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&plugins)?;
                return Ok(0);
            }
            if plugins.is_empty() {
                renderer.line(format!(
                    "no plugins installed in {}",
                    x_core::plugins::plugin_dir().display()
                ))?;
                return Ok(0);
            }
            let mut table = Table::new(["name", "path"]);
            for plugin in &plugins {
                table.push(row![plugin.name.clone(), plugin.path.display().to_string(),]);
            }
            renderer.table(&table)?;
        }
        PluginsCommand::Install { source, name } => {
            let from = std::path::PathBuf::from(source);
            if !from.is_file() {
                return Err(Error::not_found(format!(
                    "{}: no such file",
                    from.display()
                )));
            }
            let name = match name {
                Some(name) => name.clone(),
                None => from
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.trim_start_matches("x-").to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| Error::invalid_input("cannot derive a name; pass --name"))?,
            };
            let installed = x_core::plugins::install(&from, &name)
                .map_err(|e| Error::system(format!("cannot install {name}: {e}")))?;
            renderer.line(format!(
                "installed {name} -> {} (run it as `x {name}`)",
                installed.display()
            ))?;
        }
        PluginsCommand::Remove { name, yes } => {
            if !(*yes || renderer.format() == OutputFormat::Json) {
                renderer.line(format!("about to remove plugin {name}"))?;
                if !confirmer.confirm("continue?")? {
                    return Err(Error::invalid_input("aborted by user"));
                }
            }
            x_core::plugins::remove(name)
                .map_err(|e| Error::not_found(format!("cannot remove {name}: {e}")))?;
            renderer.line(format!("removed {name}"))?;
        }
    }
    Ok(0)
}

/// Run an installed plugin for an unknown `x <name> …` invocation.
///
/// Stdio is inherited so the plugin behaves exactly like a standalone tool;
/// its exit code becomes ours.
pub fn run_external(renderer: &mut Renderer, name: &str, rest: &[String]) -> Result<i32> {
    match x_core::plugins::lookup(name) {
        Some(path) => {
            let status = std::process::Command::new(path)
                .args(rest)
                .status()
                .map_err(|e| Error::system(format!("cannot run plugin {name}: {e}")))?;
            Ok(status.code().unwrap_or(1))
        }
        None => {
            renderer.line(format!(
                "unknown command `{name}` and no plugin with that name"
            ))?;
            renderer.line("see `x --help`, or `x plugins list` for what is installed")?;
            Ok(2)
        }
    }
}
