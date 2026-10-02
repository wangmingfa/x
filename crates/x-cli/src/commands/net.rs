//! `x net`: interfaces, addresses, routes, DNS, reachability probes and the
//! socket connections view.

use clap::Subcommand;
use std::collections::HashMap;
use std::net::IpAddr;
use x_core::error::{Error, Result};
use x_core::network::{format_link_speed, PingRequest};
use x_core::SystemContext;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// Default target for `x net speed`: a large, stable, publicly mirrored file.
///
/// The size matters more than the host: a few kilobytes would measure latency
/// and connection setup, not throughput. This object is the one speedtest
/// tools reach for, served over plain HTTP so no TLS handshake is timed
/// alongside the transfer.
const DEFAULT_SPEED_URL: &str = "http://speedtest.tele2.net/10MB.zip";

/// `x net` subcommands.
#[derive(Debug, Subcommand)]
pub enum NetCommand {
    /// Network interfaces, their state, link speed and default gateway.
    Interfaces,

    /// IP addresses with prefix length and DHCP flag.
    Addresses,

    /// Routing table.
    #[command(alias = "route")]
    Routes,

    /// Resolver configuration.
    Dns,

    /// Ask the platform resolver to drop its cached records.
    Flush,

    /// Every socket with its owning process (network connections view).
    Connections {
        /// Only sockets owned by processes whose name contains this text.
        #[arg(long)]
        process: Option<String>,
        /// Only sockets bound to this port.
        #[arg(long)]
        port: Option<u16>,
    },

    /// Forward resolution: host name to addresses.
    Resolve {
        /// Host name or address.
        host: String,
    },

    /// End-to-end diagnosis of one host: DNS → TCP → TLS → cert → HTTP.
    Check(crate::commands::netdiag::CheckArgs),

    /// Reverse resolution: address to host name.
    Reverse {
        /// Address to look up.
        address: IpAddr,
    },

    /// Send ICMP echo requests and summarize the replies.
    Ping {
        /// Host name or address.
        host: String,
        /// Number of echo requests to send.
        #[arg(long, default_value_t = 4)]
        count: u32,
        /// Per-request timeout in seconds.
        #[arg(long, default_value_t = 2.0)]
        timeout: f64,
    },

    /// Trace the network path to a host, one hop at a time.
    Trace {
        /// Host name or address.
        host: String,
        /// Maximum number of hops.
        #[arg(long, default_value_t = 30)]
        max_hops: u32,
        /// Per-probe timeout in seconds.
        #[arg(long, default_value_t = 2.0)]
        timeout: f64,
    },

    /// Measure download throughput against a URL.
    ///
    /// A sample of what this connection actually moved, not the negotiated
    /// link speed that `x net interfaces` reports.
    Speed {
        /// URL to fetch; the body is discarded, only the transfer is timed.
        #[arg(default_value = DEFAULT_SPEED_URL)]
        url: String,

        /// Give up after this many seconds.
        #[arg(long, default_value_t = 30.0)]
        timeout: f64,

        /// Take this many samples and report each one, so a slow start or a
        /// warm cache is visible instead of averaged away.
        #[arg(long, default_value_t = 1)]
        count: usize,
    },
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
        NetCommand::Flush => flush(context, renderer),
        NetCommand::Connections { process, port } => connections(context, renderer, process, port),
        NetCommand::Resolve { host } => resolve(context, renderer, host),
        NetCommand::Check(args) => {
            crate::commands::netdiag::dispatch_check(context, renderer, args)
        }
        NetCommand::Reverse { address } => reverse(context, renderer, *address),
        NetCommand::Ping {
            host,
            count,
            timeout,
        } => ping(context, renderer, host, *count, *timeout),
        NetCommand::Trace {
            host,
            max_hops,
            timeout,
        } => trace(context, renderer, host, *max_hops, *timeout),
        NetCommand::Speed {
            url,
            timeout,
            count,
        } => speed(renderer, url, *timeout, *count),
    }
}

/// Interfaces, with the default route marked so the user can see what carries
/// traffic without reading the whole routing table.
pub fn interfaces(context: &SystemContext, renderer: &mut Renderer) -> Result<i32> {
    let rows = context.network.interfaces()?;
    let routes = context.network.routes().unwrap_or_default();
    // The gateway of an interface is the next hop of its default route; the
    // routing table is the only place that knows it on all three platforms.
    let gateways: HashMap<String, IpAddr> = routes
        .iter()
        .filter(|route| x_core::network::is_default_route(route))
        .filter_map(|route| {
            let name = route.interface.as_ref()?;
            let gateway = route.gateway?;
            Some((name.clone(), gateway))
        })
        .collect();
    let defaults: Vec<String> = routes
        .iter()
        .filter(|route| x_core::network::is_default_route(route))
        .filter_map(|route| route.interface.clone())
        .collect();

    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(0);
    }

    let mut table = Table::new([
        "name",
        "state",
        "mac",
        "mtu",
        "speed",
        "rx",
        "tx",
        "gateway",
        "default",
        "description",
    ]);
    for row in &rows {
        let is_default = defaults.iter().any(|name| name == &row.name);
        table.push(row![
            row.name.clone(),
            state_label(row.state),
            row.mac_address.clone().unwrap_or_else(|| "-".into()),
            row.mtu.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
            row.link_speed_bps
                .map(format_link_speed)
                .unwrap_or_else(|| "-".into()),
            row.received_bytes
                .map(x_core::format_bytes)
                .unwrap_or_else(|| "-".into()),
            row.transmitted_bytes
                .map(x_core::format_bytes)
                .unwrap_or_else(|| "-".into()),
            gateways
                .get(&row.name)
                .map(|ip| ip.to_string())
                .unwrap_or_default(),
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

/// Flush the platform resolver cache.
pub fn flush(context: &SystemContext, renderer: &mut Renderer) -> Result<i32> {
    context.network.flush_dns_cache()?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&serde_json::json!({ "flushed": true }))?;
    } else {
        renderer.line("DNS cache flushed")?;
    }
    Ok(0)
}

/// Sockets with their owning process, filtered by process name or port.
pub fn connections(
    context: &SystemContext,
    renderer: &mut Renderer,
    process: &Option<String>,
    port: &Option<u16>,
) -> Result<i32> {
    let options = x_core::port::PortListOptions::default();
    let mut rows = context.port.list(&options)?;
    if let Some(term) = process {
        let needle = term.to_lowercase();
        rows.retain(|row| {
            row.process_name
                .as_deref()
                .is_some_and(|name| name.to_lowercase().contains(&needle))
        });
    }
    if let Some(port) = port {
        rows.retain(|row| row.local_port == *port);
    }

    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(0);
    }
    let mut table = Table::new(["proto", "local", "remote", "state", "pid", "process"]);
    for row in &rows {
        table.push(row![
            row.protocol.to_string(),
            row.endpoint(),
            row.remote_socket_addr()
                .map(|addr| addr.to_string())
                .unwrap_or_else(|| "-".into()),
            state_name(row.state),
            row.pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "-".into()),
            row.process_name.clone().unwrap_or_default(),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// Forward resolution of one host.
pub fn resolve(context: &SystemContext, renderer: &mut Renderer, host: &str) -> Result<i32> {
    let addresses = context.network.resolve(host)?;
    if addresses.is_empty() {
        return Err(Error::not_found(format!("no address found for `{host}`")));
    }
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&serde_json::json!({
            "host": host,
            "addresses": addresses,
        }))?;
        return Ok(0);
    }
    for address in &addresses {
        renderer.line(address.to_string())?;
    }
    Ok(0)
}

/// Reverse resolution of one address.
pub fn reverse(context: &SystemContext, renderer: &mut Renderer, address: IpAddr) -> Result<i32> {
    let name = context.network.reverse_dns(address)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&serde_json::json!({
            "address": address,
            "name": name,
        }))?;
    } else {
        renderer.line(name)?;
    }
    Ok(0)
}

/// One ICMP ping run.
pub fn ping(
    context: &SystemContext,
    renderer: &mut Renderer,
    host: &str,
    count: u32,
    timeout: f64,
) -> Result<i32> {
    let address = resolve_target(context, host)?;
    let request = PingRequest {
        address,
        count,
        timeout_ms: (timeout * 1000.0) as u32,
    };
    let summary = context.network.ping(&request)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&summary)?;
        return Ok(0);
    }
    for reply in &summary.replies {
        let ttl = reply
            .ttl
            .map(|ttl| format!(" ttl={ttl}"))
            .unwrap_or_default();
        renderer.line(format!(
            "reply from {}: seq={} time={:.1} ms{ttl}",
            reply.address, reply.sequence, reply.rtt_ms
        ))?;
    }
    let loss = if summary.transmitted == 0 {
        0.0
    } else {
        100.0 * summary.lost() as f64 / summary.transmitted as f64
    };
    let stats = match (summary.min_ms(), summary.avg_ms(), summary.max_ms()) {
        (Some(min), Some(avg), Some(max)) => {
            format!("rtt min/avg/max = {min:.1}/{avg:.1}/{max:.1} ms")
        }
        _ => "no statistics".to_string(),
    };
    renderer.line(format!(
        "{transmitted} transmitted, {received} received, {loss:.1}% loss, {stats}",
        transmitted = summary.transmitted,
        received = summary.replies.len(),
    ))?;
    // A run without a single reply is a failure, matching `ping` semantics.
    if summary.replies.is_empty() {
        return Ok(1);
    }
    Ok(0)
}

/// One traceroute run.
pub fn trace(
    context: &SystemContext,
    renderer: &mut Renderer,
    host: &str,
    max_hops: u32,
    timeout: f64,
) -> Result<i32> {
    let address = resolve_target(context, host)?;
    let hops = context
        .network
        .trace(address, max_hops, (timeout * 1000.0) as u32)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&serde_json::json!({
            "host": host,
            "address": address,
            "hops": hops,
        }))?;
        return Ok(0);
    }
    let mut table = Table::new(["hop", "address", "rtt"]);
    for hop in &hops {
        table.push(row![
            hop.index.to_string(),
            hop.address
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "*".into()),
            hop.rtt_ms
                .map(|rtt| format!("{rtt:.1} ms"))
                .unwrap_or_default(),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// Measure download throughput by timing a transfer.
///
/// Each sample is reported on its own rather than averaged: a slow first run
/// (TLS-less connection setup, cold route, cold CDN edge) and a warm second one
/// answer different questions, and an average hides both. A failed sample ends
/// the run with the tool's own error rather than printing a rate that never
/// happened.
fn speed(renderer: &mut Renderer, url: &str, timeout: f64, count: usize) -> Result<i32> {
    let timeout_ms = if timeout.is_finite() && timeout > 0.0 {
        (timeout * 1000.0) as u64
    } else {
        30_000
    };
    let samples = count.max(1);

    let mut taken: Vec<x_core::netdiag::SpeedSample> = Vec::new();
    for index in 0..samples {
        let sample = x_platform::common::netdiag::speed_sample(url, timeout_ms)?;
        if renderer.format() == OutputFormat::Json {
            renderer.always_json(&sample)?;
        } else if samples == 1 {
            renderer.line(format!(
                "{url}\n  {}  ({:.2}s, {})",
                sample.human_bits_per_second(),
                sample.seconds,
                x_core::format_bytes(sample.bytes),
            ))?;
        } else {
            renderer.line(format!(
                "[{}/{}] {}  ({:.2}s, {})",
                index + 1,
                samples,
                sample.human_bits_per_second(),
                sample.seconds,
                x_core::format_bytes(sample.bytes),
            ))?;
        }
        taken.push(sample);
    }

    // The peak is reported alongside the list because what a user usually
    // wants is "how fast can this link go", and the best run is the closest
    // answer that still came from a real measurement.
    if renderer.format() != OutputFormat::Json && taken.len() > 1 {
        if let Some(best) = taken.iter().max_by_key(|s| s.bits_per_second) {
            renderer.line(format!(
                "peak {} from {} sample(s)",
                best.human_bits_per_second(),
                taken.len()
            ))?;
        }
    }
    Ok(0)
}

/// Turn a CLI host argument into an address: literal addresses pass through,
/// names go through the platform resolver.
fn resolve_target(context: &SystemContext, host: &str) -> Result<IpAddr> {
    if let Ok(address) = host.parse::<IpAddr>() {
        return Ok(address);
    }
    context
        .network
        .resolve(host)?
        .into_iter()
        .next()
        .ok_or_else(|| Error::not_found(format!("no address found for `{host}`")))
}

/// Serde name of a connection state, used as the table label.
fn state_name(state: x_core::port::ConnectionState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

fn state_label(state: x_core::network::InterfaceState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}
