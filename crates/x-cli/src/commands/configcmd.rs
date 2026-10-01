//! `x config`: show where the config lives and what x honours from it.

use clap::Subcommand;
use x_core::config;
use x_core::error::Result;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x config` subcommands.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the config file path (creating nothing).
    Path,

    /// Show the effective config plus any problems found while parsing.
    Show,
}

/// Arguments for `x config`.
#[derive(Debug, clap::Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

/// Route a `x config` invocation.
pub fn dispatch(
    _context: &x_core::SystemContext,
    renderer: &mut Renderer,
    command: &ConfigCommand,
) -> Result<i32> {
    let path = config::default_path();
    match command {
        ConfigCommand::Path => {
            renderer.line(path.display().to_string())?;
        }
        ConfigCommand::Show => {
            let (cfg, warnings) = config::load(&path);
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&serde_json::json!({
                    "path": path.display().to_string(),
                    "config": cfg,
                    "warnings": warnings,
                }))?;
                return Ok(0);
            }
            renderer.line(format!("file: {}", path.display()))?;
            let mut table = Table::new(["key", "value"]);
            table.push(row![
                "default_format",
                cfg.default_format.clone().unwrap_or_else(|| "table".into()),
            ]);
            table.push(row![
                "refresh_ms",
                cfg.refresh_ms
                    .map(|ms| ms.to_string())
                    .unwrap_or_else(|| "platform default".into()),
            ]);
            table.push(row![
                "theme",
                cfg.theme.clone().unwrap_or_else(|| "default".into()),
            ]);
            table.push(row![
                "sort",
                cfg.sort
                    .clone()
                    .unwrap_or_else(|| "platform default".into()),
            ]);
            renderer.table(&table)?;
            for warning in &warnings {
                renderer.line(format!("warning: {warning}"))?;
            }
        }
    }
    Ok(0)
}
