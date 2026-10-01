//! `x firewall`: the platform firewall's state and port rules.
//!
//! `status` / `list` are read-only. `allow` / `deny` change the machine and
//! need elevation on every platform, so they are confirmed first and the
//! adapters attach the structured permission requirement to a refusal.

use clap::Subcommand;
use serde_json::json;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{Confirmer, OutputFormat, Renderer, Table},
    row,
};

/// `x firewall` subcommands.
#[derive(Debug, Subcommand)]
pub enum FirewallCommand {
    /// Which stack answers and whether it is on.
    Status,

    /// Visible rules.
    List {
        /// Maximum number of rows.
        #[arg(long)]
        limit: Option<usize>,
    },

    /// Open an inbound port (needs administrator / root).
    Allow(RuleArgs),

    /// Block a port (needs administrator / root).
    Deny(RuleArgs),
}

/// Arguments shared by `allow` and `deny`.
#[derive(Debug, clap::Args)]
pub struct RuleArgs {
    /// Port to open or block.
    pub port: u16,
    /// Protocol of the rule.
    #[arg(long, value_parser = ["tcp", "udp"], default_value = "tcp")]
    pub proto: String,
    /// Rule name in the firewall's own store.
    #[arg(long)]
    pub name: Option<String>,
    /// Do not ask for confirmation.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for `x firewall`.
#[derive(Debug, clap::Args)]
pub struct FirewallArgs {
    #[command(subcommand)]
    pub command: FirewallCommand,
}

/// Route a `x firewall` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    command: &FirewallCommand,
) -> Result<i32> {
    let firewall = context
        .firewall
        .as_ref()
        .ok_or_else(|| Error::unsupported("no firewall capability in this context"))?;

    match command {
        FirewallCommand::Status => {
            let enabled = firewall.enabled()?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&json!({
                    "stack": firewall.stack().name(),
                    "enabled": enabled,
                }))?;
                return Ok(0);
            }
            let mut table = Table::new(["field", "value"]);
            table.push(row!["stack", firewall.stack().name().to_string()]);
            table.push(row![
                "enabled",
                if enabled { "yes" } else { "no" }.to_string()
            ]);
            renderer.table(&table)?;
        }
        FirewallCommand::List { limit } => {
            let mut rows = firewall.list()?;
            let total = rows.len();
            if let Some(limit) = limit {
                rows.truncate(*limit);
            }
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&rows)?;
                return Ok(0);
            }
            let mut table = Table::new(["name", "action", "port", "protocol", "enabled"]);
            for rule in &rows {
                table.push(row![
                    rule.name.clone(),
                    rule.action.clone(),
                    rule.port.clone().unwrap_or_else(|| "-".into()),
                    rule.protocol.clone().unwrap_or_else(|| "-".into()),
                    if rule.enabled { "✓" } else { "✗" }.to_string(),
                ]);
            }
            renderer.table(&table)?;
            if rows.len() < total {
                renderer.line(format!(
                    "… {} more rule(s) (use --limit)",
                    total - rows.len()
                ))?;
            }
        }
        FirewallCommand::Allow(args) => {
            confirm(
                renderer,
                confirmer,
                args.yes,
                &format!("open {} port {}", args.proto, args.port),
            )?;
            firewall.allow(args.port, Some(&args.proto), args.name.as_deref())?;
            renderer.line(format!("allowed inbound {} port {}", args.proto, args.port))?;
        }
        FirewallCommand::Deny(args) => {
            confirm(
                renderer,
                confirmer,
                args.yes,
                &format!("block {} port {}", args.proto, args.port),
            )?;
            firewall.deny(args.port, Some(&args.proto), args.name.as_deref())?;
            renderer.line(format!("blocked inbound {} port {}", args.proto, args.port))?;
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
