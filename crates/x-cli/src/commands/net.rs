//! `x net`: interfaces, addresses, routes and DNS.

use clap::Subcommand;
use x_core::error::Result;
use x_core::SystemContext;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x net` subcommands.
#[derive(Debug, Subcommand)]
pub enum NetCommand {
    /// Network interfaces and their state.
    Interfaces,

    /// IP addresses with prefix length and DHCP flag.
    Addresses,

    /// Routing table.
    #[command(alias = "route")]
    Routes,

    /// Resolver configuration.
    Dns,
}

/// Route a `x net` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &NetCommand,
) -> Result<i32> {
    match command {
        NetCommand::Interfaces => interfaces(context, renderer),
        NetCommand::Addresses => addresses(context, renderer),
        NetCommand::Routes => routes(context, renderer),
        NetCommand::Dns => dns(context, renderer),
    }
}

/// Interfaces, with the default route marked so the user can see what carries
/// traffic without reading the whole routing table.
pub fn interfaces(context: &SystemContext, renderer: &mut Renderer) -> Result<i32> {
    let rows = context.network.interfaces()?;
    let defaults: Vec<String> = context
        .network
        .routes()
        .unwrap_or_default()
        .iter()
        .filter_map(|route| route.interface.clone())
        .collect();

    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(0);
    }

    let mut table = Table::new(["name", "state", "mac", "mtu", "default", "description"]);
    for row in &rows {
        let is_default = defaults.iter().any(|name| name == &row.name);
        table.push(row![
            row.name.clone(),
            state_label(row.state),
            row.mac_address.clone().unwrap_or_else(|| "-".into()),
            row.mtu.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
            if is_default { "yes" } else { "" }.to_string(),
            row.description.clone().unwrap_or_default(),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// Addresses.
pub fn addresses(context: &SystemContext, renderer: &mut Renderer) -> Result<i32> {
    let rows = context.network.addresses()?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(0);
    }
    let mut table = Table::new(["interface", "address", "prefix", "dhcp"]);
    for row in &rows {
        table.push(row![
            row.interface.clone(),
            row.address.to_string(),
            row.prefix_len.map(|v| v.to_string()).unwrap_or_default(),
            match row.dhcp {
                Some(true) => "yes",
                Some(false) => "no",
                None => "",
            }
            .to_string(),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// Routes, default route first.
pub fn routes(context: &SystemContext, renderer: &mut Renderer) -> Result<i32> {
    let mut rows = context.network.routes()?;
    rows.sort_by_key(|route| !x_core::network::is_default_route(route));
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(0);
    }
    let mut table = Table::new(["destination", "gateway", "interface", "metric"]);
    for row in &rows {
        table.push(row![
            row.destination.clone().unwrap_or_else(|| "default".into()),
            row.gateway
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "on-link".into()),
            row.interface.clone().unwrap_or_else(|| "-".into()),
            row.metric.map(|v| v.to_string()).unwrap_or_default(),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// DNS resolvers and search domains.
pub fn dns(context: &SystemContext, renderer: &mut Renderer) -> Result<i32> {
    let config = context.network.dns()?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&config)?;
        return Ok(0);
    }
    if config.servers.is_empty() {
        renderer.line("no DNS server configured")?;
        return Ok(0);
    }

    let mut table = Table::new(["server", "interface"]);
    for row in &config.servers {
        table.push(row![
            row.address.to_string(),
            row.interface.clone().unwrap_or_else(|| "-".into()),
        ]);
    }
    renderer.table(&table)?;

    if !config.search_domains.is_empty() {
        let mut domains = config.search_domains.clone();
        domains.dedup();
        renderer.line(format!("search: {}", domains.join(", ")))?;
    }
    Ok(0)
}

fn state_label(state: x_core::network::InterfaceState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}
