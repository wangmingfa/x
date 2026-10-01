//! `x proxy`: inspect environment and system proxy, edit the env layer.

use clap::Subcommand;
use x_core::error::Result;
use x_core::SystemContext;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x proxy` subcommands.
#[derive(Debug, Subcommand)]
pub enum ProxyCommand {
    /// Show the environment proxy and the OS-level proxy.
    Get,

    /// Set HTTP_PROXY / HTTPS_PROXY / NO_PROXY for this process and children.
    Set {
        /// Proxy address, e.g. `http://127.0.0.1:7890`.
        address: String,
        /// Comma separated exceptions for NO_PROXY.
        #[arg(long)]
        no_proxy: Option<String>,
    },

    /// Remove every proxy variable from the environment.
    Clear,
}

/// Arguments for `x proxy`.
#[derive(Debug, clap::Args)]
pub struct ProxyArgs {
    #[command(subcommand)]
    pub command: ProxyCommand,
}

/// Route a `x proxy` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &ProxyCommand,
) -> Result<i32> {
    match command {
        ProxyCommand::Get => {
            let env = context.proxy.as_ref().map(|p| p.env()).unwrap_or_default();
            let system = context.proxy.as_ref().and_then(|p| p.system());
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&serde_json::json!({
                    "env": env,
                    "system": system,
                }))?;
                return Ok(0);
            }
            let mut table = Table::new(["layer", "http", "https", "no_proxy"]);
            let dash = "-".to_string();
            table.push(row![
                "env",
                env.http.clone().unwrap_or_else(|| dash.clone()),
                env.https.clone().unwrap_or_else(|| dash.clone()),
                env.no_proxy.clone().unwrap_or_else(|| dash.clone()),
            ]);
            if let Some(system) = &system {
                let status = if system.enabled { "on" } else { "off" };
                table.push(row![
                    format!("system ({status})"),
                    system.http.clone().unwrap_or_else(|| dash.clone()),
                    system.https.clone().unwrap_or_else(|| dash.clone()),
                    system.exceptions.clone().unwrap_or_else(|| dash.clone()),
                ]);
                if let Some(how) = &system.how_to_change {
                    renderer.line(format!("change via: {how}"))?;
                }
            }
            renderer.table(&table)?;
        }
        ProxyCommand::Set { address, no_proxy } => {
            let cfg = x_core::proxy::EnvProxy {
                http: Some(address.clone()),
                https: Some(address.clone()),
                no_proxy: no_proxy.clone(),
            };
            cfg.apply_env();
            renderer.line(format!(
                "HTTP_PROXY/HTTPS_PROXY={address} (this process only; export in your shell profile to persist)"
            ))?;
        }
        ProxyCommand::Clear => {
            x_core::proxy::EnvProxy::clear_env();
            renderer.line("proxy variables cleared from the environment (this process only)")?;
        }
    }
    Ok(0)
}
