//! `x net`: interfaces, addresses, routes, DNS, reachability probes and the
//! socket connections view.

use clap::Subcommand;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;
use x_core::error::{Error, Result};
use x_core::format_bytes;
use x_core::net_top::{diff, NetSnapshot, NetTopReport};
use x_core::network::{format_link_speed, PingRequest};
use x_core::SystemContext;
use x_platform::PlatformNetSampler;

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

    /// Live per-process network usage: rates, connections and their sources.
    Top(TopArgs),
}

/// `x net top` arguments.
#[derive(Debug, clap::Args)]
pub struct TopArgs {
    /// Seconds between rounds; also the capture window.
    #[arg(long, default_value_t = 2.0)]
    pub interval: f64,

    /// Stop after this many rounds instead of running until interrupted.
    #[arg(long)]
    pub count: Option<usize>,

    /// Only show this process id (the unmapped row is hidden).
    #[arg(long)]
    pub pid: Option<i32>,

    /// Sort key: rx, tx, conns or pid.
    #[arg(long, default_value = "rx")]
    pub sort: String,
}

/// Route a `x net` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &NetCommand,
) -> Result<i32> {
    match command {
        NetCommand::Interfaces => interfaces(context, renderer),
        NetCommand::Top(args) => top(context, renderer, args),
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

/// Live per-process network usage: poll the platform sampler, diff
/// consecutive rounds and render rates, connections and their sources —
/// the same snapshot-diff loop `x events` runs. The capture window inside
/// `sample` paces rounds on platforms that capture, so there is no extra
/// sleep there.
fn top(context: &SystemContext, renderer: &mut Renderer, args: &TopArgs) -> Result<i32> {
    let interval = if args.interval.is_finite() && args.interval > 0.0 {
        args.interval.max(0.05)
    } else {
        2.0
    };
    let window = Duration::from_secs_f64(interval);
    let sort = SortKey::parse(&args.sort)?;
    let json = renderer.format() == OutputFormat::Json;

    let sampler = PlatformNetSampler::new();

    let mut previous = NetSnapshot::default();
    let mut rounds = 0usize;
    loop {
        let current = sampler.sample(context, window);

        // Exit 7 when not a single layer came back: every platform can read
        // the connection table unprivileged, so both layers failing means
        // this host has nothing to show.
        if rounds == 0 && current.interfaces.is_none() && current.connections.is_none() {
            return Err(Error::unsupported(
                "no network layer could be read on this host (interfaces and connections both failed)",
            ));
        }

        let report = diff(&previous, &current, window);
        if json {
            render_top_json(renderer, &report, &sampler, args, sort)?;
        } else {
            render_top_table(renderer, &report, &sampler, args, sort, rounds)?;
        }
        renderer.flush().map_err(|e| {
            Error::new(
                x_core::ErrorKind::System,
                format!("net top output failed: {e}"),
            )
        })?;

        previous = current;
        rounds += 1;
        if args.count.is_some_and(|target| rounds >= target) {
            return Ok(0);
        }
        // Platforms without a capture layer return from `sample` instantly;
        // sleep there so the loop keeps the requested interval.
        if !sampler.per_process_available() {
            std::thread::sleep(window);
        }
    }
}

/// One renderable row: a process rate plus the name resolved from the
/// sampler's most recent socket table.
struct TopRow {
    pid: i32,
    process: String,
    rx_bps: Option<f64>,
    tx_bps: Option<f64>,
    conns: usize,
    source: &'static str,
}

/// Column `--sort` orders by; ties break by pid ascending.
#[derive(Debug, Clone, Copy)]
enum SortKey {
    Rx,
    Tx,
    Conns,
    Pid,
}

impl SortKey {
    fn parse(key: &str) -> Result<Self> {
        match key {
            "rx" => Ok(Self::Rx),
            "tx" => Ok(Self::Tx),
            "conns" => Ok(Self::Conns),
            "pid" => Ok(Self::Pid),
            other => Err(Error::invalid_input(format!(
                "unknown sort key `{other}` (expected rx, tx, conns or pid)"
            ))),
        }
    }

    fn order(self, rows: &mut [TopRow]) {
        match self {
            // Rates sort descending with absent values last: a process we
            // could not measure never outranks one we did.
            Self::Rx => rows.sort_by(|a, b| {
                b.rx_bps
                    .partial_cmp(&a.rx_bps)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.pid.cmp(&b.pid))
            }),
            Self::Tx => rows.sort_by(|a, b| {
                b.tx_bps
                    .partial_cmp(&a.tx_bps)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.pid.cmp(&b.pid))
            }),
            Self::Conns => rows.sort_by(|a, b| b.conns.cmp(&a.conns).then(a.pid.cmp(&b.pid))),
            Self::Pid => rows.sort_by_key(|row| row.pid),
        }
    }
}

/// The report's process rows, name-resolved, `--pid`-filtered and sorted.
fn sorted_top_rows(
    report: &NetTopReport,
    sampler: &PlatformNetSampler,
    args: &TopArgs,
    sort: SortKey,
) -> Vec<TopRow> {
    let mut rows: Vec<TopRow> = report
        .processes
        .iter()
        .filter(|rate| args.pid.is_none_or(|pid| rate.pid == pid))
        .map(|rate| TopRow {
            pid: rate.pid,
            process: sampler
                .process_name(rate.pid)
                .unwrap_or_else(|| "?".to_string()),
            rx_bps: rate.rx_bps,
            tx_bps: rate.tx_bps,
            conns: rate.conns,
            source: rate.source,
        })
        .collect();
    sort.order(&mut rows);
    rows
}

/// Rates render as human bytes per second; an absent rate stays `-` instead
/// of a fabricated zero.
fn rate_cell(rate: Option<f64>) -> String {
    match rate {
        Some(bps) => format!("{}/s", format_bytes(bps as u64)),
        None => "-".to_string(),
    }
}

fn render_top_table(
    renderer: &mut Renderer,
    report: &NetTopReport,
    sampler: &PlatformNetSampler,
    args: &TopArgs,
    sort: SortKey,
    round: usize,
) -> Result<()> {
    if round == 0 {
        let source = if sampler.per_process_available() {
            "port-inference (packet capture)".to_string()
        } else {
            sampler
                .per_process_error()
                .unwrap_or("unavailable on this platform")
                .to_string()
        };
        renderer.line(format!(
            "net top: sampling every {}s; per-process source: {source}",
            report.interval.as_secs_f64()
        ))?;
        // Only when privileges are the reason: on a platform with no source
        // at all, sudo changes nothing and the hint would mislead.
        if sampler.per_process_denied() {
            renderer.line("hint: re-run with sudo to enable per-process rates")?;
        }
    }
    let mut table = Table::new(["PID", "PROCESS", "RX/s", "TX/s", "CONNS", "SOURCE"]);
    for row in sorted_top_rows(report, sampler, args, sort) {
        table.push(row![
            row.pid.to_string(),
            row.process,
            rate_cell(row.rx_bps),
            rate_cell(row.tx_bps),
            row.conns.to_string(),
            row.source,
        ]);
    }
    // Unmapped = host total minus the attributed sum; it is derived, not a
    // process, so it renders last without a pid — and only when the host
    // counters that derive it exist.
    if args.pid.is_none() && (report.unmapped.rx_bps.is_some() || report.unmapped.tx_bps.is_some())
    {
        table.push(row![
            "-",
            "(unmapped)",
            rate_cell(report.unmapped.rx_bps),
            rate_cell(report.unmapped.tx_bps),
            "-",
            "host counters only",
        ]);
    }
    renderer.table(&table)?;
    for (layer, reason) in &report.failures {
        renderer.line(format!("! {layer}: {reason}"))?;
    }
    if round == 0 {
        renderer.line("collecting baseline: per-process rates appear from the next round")?;
    }
    Ok(())
}

fn render_top_json(
    renderer: &mut Renderer,
    report: &NetTopReport,
    sampler: &PlatformNetSampler,
    args: &TopArgs,
    sort: SortKey,
) -> Result<()> {
    let mut doc = serde_json::Map::new();
    doc.insert(
        "interval_s".into(),
        serde_json::json!(report.interval.as_secs_f64()),
    );
    // A layer that failed this round leaves its whole object out: JSON must
    // not render an absent reading as null-or-zero.
    if let (Some(rx), Some(tx)) = (report.host_rx_bps, report.host_tx_bps) {
        doc.insert(
            "host".into(),
            serde_json::json!({ "rx_bps": rx, "tx_bps": tx }),
        );
    }
    let processes: Vec<serde_json::Value> = sorted_top_rows(report, sampler, args, sort)
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            obj.insert("pid".into(), serde_json::json!(row.pid));
            obj.insert("process".into(), serde_json::json!(row.process));
            if let Some(rx) = row.rx_bps {
                obj.insert("rx_bps".into(), serde_json::json!(rx));
            }
            if let Some(tx) = row.tx_bps {
                obj.insert("tx_bps".into(), serde_json::json!(tx));
            }
            obj.insert("conns".into(), serde_json::json!(row.conns));
            obj.insert("source".into(), serde_json::json!(row.source));
            serde_json::Value::Object(obj)
        })
        .collect();
    doc.insert("processes".into(), serde_json::Value::Array(processes));
    if args.pid.is_none() {
        if let (Some(rx), Some(tx)) = (report.unmapped.rx_bps, report.unmapped.tx_bps) {
            doc.insert(
                "unmapped".into(),
                serde_json::json!({ "rx_bps": rx, "tx_bps": tx }),
            );
        }
    }
    if !report.failures.is_empty() {
        let failures: Vec<serde_json::Value> = report
            .failures
            .iter()
            .map(|(layer, reason)| serde_json::json!({ "layer": layer, "reason": reason }))
            .collect();
        doc.insert("failures".into(), serde_json::Value::Array(failures));
    }
    renderer.always_json(&serde_json::Value::Object(doc))?;
    Ok(())
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
