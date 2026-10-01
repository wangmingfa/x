//! `x env` / `x path` / `x which`: environment and command lookup.
//!
//! These are pure process-environment work — no platform adapter involved.
//! `x env set` and `x path add/remove` change the current process only; making
//! a variable permanent is shell-profile territory and the output says so.

use clap::Subcommand;
use x_core::error::Result;
use x_core::SystemContext;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x env` subcommands.
#[derive(Debug, Subcommand)]
pub enum EnvCommand {
    /// List all environment variables, sorted by name.
    List {
        /// Case insensitive substring filter on the name.
        #[arg(long)]
        search: Option<String>,
    },

    /// Print one variable's value.
    Get {
        /// Variable name.
        name: String,
    },

    /// Set one variable for this process and its children.
    Set {
        /// Variable name.
        name: String,
        /// New value.
        value: String,
    },
}

/// Arguments for `x env`.
#[derive(Debug, clap::Args)]
pub struct EnvArgs {
    #[command(subcommand)]
    pub command: EnvCommand,
}

/// Route an `x env` invocation.
pub fn dispatch_env(
    _context: &SystemContext,
    renderer: &mut Renderer,
    command: &EnvCommand,
) -> Result<i32> {
    match command {
        EnvCommand::List { search } => {
            let mut vars = x_core::devenv::env_list();
            if let Some(needle) = search {
                let needle = needle.to_ascii_lowercase();
                vars.retain(|v| v.name.to_ascii_lowercase().contains(&needle));
            }
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&vars)?;
                return Ok(0);
            }
            let mut table = Table::new(["name", "value"]);
            for var in &vars {
                table.push(row![var.name.clone(), var.value.clone()]);
            }
            renderer.table(&table)?;
        }
        EnvCommand::Get { name } => {
            let value = x_core::devenv::env_get(name)?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&serde_json::json!({ "name": name, "value": value }))?;
            } else {
                renderer.line(value)?;
            }
        }
        EnvCommand::Set { name, value } => {
            x_core::devenv::env_set(name, value)?;
            renderer.line(format!(
                "{name}={value} (this process only; add it to your shell profile to persist)"
            ))?;
        }
    }
    Ok(0)
}

/// `x path` subcommands.
#[derive(Debug, Subcommand)]
pub enum PathCommand {
    /// List every `PATH` entry and whether it exists.
    List {
        /// Also list the commands found in each entry.
        #[arg(long)]
        commands: bool,
    },

    /// Find which `PATH` entries provide a command.
    Find {
        /// Command name (`node` finds `node.exe` on Windows too).
        name: String,
    },

    /// Add a directory to `PATH` for this process and its children.
    Add {
        /// Directory to prepend.
        dir: String,
    },

    /// Remove a directory from `PATH` for this process and its children.
    Remove {
        /// Directory to drop.
        dir: String,
    },
}

/// Arguments for `x path`.
#[derive(Debug, clap::Args)]
pub struct PathArgs {
    #[command(subcommand)]
    pub command: PathCommand,
}

/// Route a `x path` invocation.
pub fn dispatch_path(
    _context: &SystemContext,
    renderer: &mut Renderer,
    command: &PathCommand,
) -> Result<i32> {
    match command {
        PathCommand::List { commands } => {
            let entries = x_core::devenv::path_entries();
            if renderer.format() == OutputFormat::Json {
                if *commands {
                    renderer.always_json(&entries)?;
                } else {
                    let dirs: Vec<&str> = entries.iter().map(|e| e.dir.as_str()).collect();
                    renderer.always_json(&dirs)?;
                }
                return Ok(0);
            }
            let mut table = Table::new(["dir", "exists"]);
            if *commands {
                for entry in &entries {
                    table.push(row![
                        entry.dir.clone(),
                        entry.exists.to_string(),
                        entry.commands.join(" "),
                    ]);
                }
            } else {
                for entry in &entries {
                    table.push(row![entry.dir.clone(), entry.exists.to_string()]);
                }
            }
            renderer.table(&table)?;
        }
        PathCommand::Find { name } => {
            let hits = x_core::devenv::path_find(name);
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&hits)?;
                return Ok(0);
            }
            if hits.is_empty() {
                renderer.line(format!("{name}: not found on PATH"))?;
                return Ok(1);
            }
            let mut table = Table::new(["path"]);
            for hit in &hits {
                table.push(row![hit.display().to_string()]);
            }
            renderer.table(&table)?;
        }
        PathCommand::Add { dir } => {
            let current = std::env::var("PATH").unwrap_or_default();
            let separator = x_core::devenv::path_separator();
            std::env::set_var("PATH", format!("{dir}{separator}{current}"));
            renderer.line(format!(
                "prepended {dir} to PATH (this process only; edit your shell profile to persist)"
            ))?;
        }
        PathCommand::Remove { dir } => {
            let current = std::env::var("PATH").unwrap_or_default();
            let separator = x_core::devenv::path_separator();
            let kept: Vec<&str> = current
                .split(separator)
                .filter(|entry| !entry.is_empty() && *entry != dir.as_str())
                .collect();
            std::env::set_var("PATH", kept.join(&separator.to_string()));
            renderer.line(format!(
                "removed {dir} from PATH (this process only; edit your shell profile to persist)"
            ))?;
        }
    }
    Ok(0)
}

/// Arguments for `x which`.
#[derive(Debug, clap::Args)]
pub struct WhichArgs {
    /// Command names to resolve.
    pub commands: Vec<String>,
}

/// Route a `x which` invocation.
pub fn dispatch_which(
    _context: &SystemContext,
    renderer: &mut Renderer,
    args: &WhichArgs,
) -> Result<i32> {
    if renderer.format() == OutputFormat::Json {
        let hits: Vec<serde_json::Value> = args
            .commands
            .iter()
            .map(|command| {
                serde_json::json!({
                    "command": command,
                    "path": x_core::devenv::which(command)
                        .map(|p| p.display().to_string()),
                })
            })
            .collect();
        renderer.always_json(&hits)?;
        return Ok(0);
    }
    let mut table = Table::new(["command", "path"]);
    let mut missing = 0;
    for command in &args.commands {
        let path = x_core::devenv::which(command);
        if path.is_none() {
            missing += 1;
        }
        table.push(row![
            command.clone(),
            path.map(|p| p.display().to_string())
                .unwrap_or_else(|| "not found".into()),
        ]);
    }
    renderer.table(&table)?;
    Ok(if missing > 0 && args.commands.len() == 1 {
        1
    } else {
        0
    })
}
