//! `x cert` / `x tls` / `x http` / `x headers` and the `x net check` chain.
//!
//! All of it is outbound reads through the platform's own tools — no
//! confirmation, no elevation, no audit line. A field the tool did not print
//! stays absent; a stage the chain could not run is reported as such instead
//! of guessed.

use clap::Subcommand;
use std::net::IpAddr;
use std::time::Duration;
use x_core::error::{Error, ErrorKind, Result};
use x_core::netdiag::{DiagStep, HttpResponse, TlsInfo};
use x_core::SystemContext;

use crate::format::{OutputFormat, Renderer, Table};
use crate::row;

/// `x cert` subcommands.
#[derive(Debug, Subcommand)]
pub enum CertCommand {
    /// TLS handshake plus leaf-certificate facts for a host.
    Check(CertArgs),
}

/// Arguments shared by `x cert check` and `x tls`.
#[derive(Debug, clap::Args)]
pub struct CertArgs {
    /// Host name or address.
    pub host: String,
    /// TLS port.
    #[arg(long, default_value_t = 443)]
    pub port: u16,
    /// Probe timeout in seconds.
    #[arg(long, default_value_t = 5)]
    pub timeout: u64,
}

/// `x tls` arguments: the handshake view of `x cert check`.
#[derive(Debug, clap::Args)]
pub struct TlsArgs {
    #[command(flatten)]
    pub args: CertArgs,
}

/// Arguments for `x http` and `x headers`.
#[derive(Debug, clap::Args)]
pub struct HttpArgs {
    /// Absolute URL, e.g. `https://example.com`.
    pub url: String,
    /// Request method.
    #[arg(long = "method", short = 'X', default_value = "GET")]
    pub method: String,
    /// Request timeout in seconds.
    #[arg(long, default_value_t = 5)]
    pub timeout: u64,
}

/// Arguments for `x net check`.
#[derive(Debug, clap::Args)]
pub struct CheckArgs {
    /// Host name or address.
    pub host: String,
    /// Port to probe (TLS stages; the HTTP stage picks its scheme from it).
    #[arg(long, default_value_t = 443)]
    pub port: u16,
    /// Per-probe timeout in seconds.
    #[arg(long, default_value_t = 5)]
    pub timeout: u64,
}

/// Route `x cert`.
pub fn dispatch_cert(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &CertCommand,
) -> Result<i32> {
    match command {
        CertCommand::Check(args) => cert_check(context, renderer, args),
    }
}

/// Route `x tls`: the handshake view of the same probe.
pub fn dispatch_tls(
    context: &SystemContext,
    renderer: &mut Renderer,
    args: &TlsArgs,
) -> Result<i32> {
    let args = &args.args;
    let info = probe_tls(context, &args.host, args.port, args.timeout)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&info)?;
        return Ok(0);
    }
    let mut table = Table::new(["field", "value"]);
    table.push(row!["endpoint", format!("{}:{}", info.host, info.port)]);
    push_if_some(&mut table, "protocol", &info.protocol);
    push_if_some(&mut table, "cipher", &info.cipher);
    table.push(row![
        "verified",
        verdict_text(info.verified, &info.verify_detail)
    ]);
    renderer.table(&table)?;
    Ok(0)
}

/// Route `x http`: status and timings of one request.
pub fn dispatch_http(
    context: &SystemContext,
    renderer: &mut Renderer,
    args: &HttpArgs,
) -> Result<i32> {
    let response = probe_http(context, args)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&response)?;
        return Ok(0);
    }
    let mut table = Table::new(["field", "value"]);
    table.push(row!["url", response.url.clone()]);
    table.push(row![
        "status",
        response
            .status
            .map(|code| code.to_string())
            .unwrap_or_else(|| "-".into())
    ]);
    push_if_some(&mut table, "version", &response.http_version);
    push_if_some(
        &mut table,
        "time_ms",
        &response.time_total_ms.map(|ms| format!("{ms:.0}")),
    );
    push_if_some(&mut table, "remote_ip", &response.remote_ip);
    push_if_some(
        &mut table,
        "bytes",
        &response.bytes.map(|bytes| bytes.to_string()),
    );
    renderer.table(&table)?;
    Ok(0)
}

/// Route `x headers`: the response header block.
pub fn dispatch_headers(
    context: &SystemContext,
    renderer: &mut Renderer,
    args: &HttpArgs,
) -> Result<i32> {
    let response = probe_http(context, args)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&response.headers)?;
        return Ok(0);
    }
    let mut table = Table::new(["header", "value"]);
    for (key, value) in &response.headers {
        table.push(row![key.clone(), value.clone()]);
    }
    renderer.table(&table)?;
    renderer.line(format!(
        "status {} in {} ms",
        response
            .status
            .map(|code| code.to_string())
            .unwrap_or_else(|| "-".into()),
        response
            .time_total_ms
            .map(|ms| format!("{ms:.0}"))
            .unwrap_or_else(|| "-".into())
    ))?;
    Ok(0)
}

/// `x net check`: the DNS → TCP → TLS → cert → HTTP chain.
pub fn dispatch_check(
    context: &SystemContext,
    renderer: &mut Renderer,
    args: &CheckArgs,
) -> Result<i32> {
    let steps = run_chain(context, args)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&steps)?;
        return Ok(if DiagStep::all_ok(&steps) { 0 } else { 1 });
    }
    let mut table = Table::new(["stage", "verdict", "detail"]);
    for step in &steps {
        table.push(row![
            step.stage.clone(),
            format!("{} {}", step.mark(), step.verdict),
            step.detail.clone(),
        ]);
    }
    renderer.table(&table)?;
    Ok(if DiagStep::all_ok(&steps) { 0 } else { 1 })
}

fn cert_check(context: &SystemContext, renderer: &mut Renderer, args: &CertArgs) -> Result<i32> {
    let info = probe_tls(context, &args.host, args.port, args.timeout)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&info)?;
        return Ok(0);
    }
    let mut table = Table::new(["field", "value"]);
    table.push(row!["endpoint", format!("{}:{}", info.host, info.port)]);
    push_if_some(&mut table, "protocol", &info.protocol);
    push_if_some(&mut table, "cipher", &info.cipher);
    push_if_some(&mut table, "subject", &info.subject);
    push_if_some(&mut table, "issuer", &info.issuer);
    push_if_some(&mut table, "not_before", &info.not_before);
    push_if_some(&mut table, "not_after", &info.not_after);
    if !info.san.is_empty() {
        table.push(row!["san", info.san.join(", ")]);
    }
    table.push(row![
        "verified",
        verdict_text(info.verified, &info.verify_detail)
    ]);
    renderer.table(&table)?;
    Ok(0)
}

fn probe_tls(context: &SystemContext, host: &str, port: u16, timeout: u64) -> Result<TlsInfo> {
    context
        .network
        .tls_info(host, port, timeout * 1000)
        .map(|mut info| {
            if info.host.is_empty() {
                info.host = host.to_string();
            }
            info
        })
}

fn probe_http(context: &SystemContext, args: &HttpArgs) -> Result<HttpResponse> {
    if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
        return Err(Error::invalid_input(format!(
            "`{}` is not an http(s) URL",
            args.url
        )));
    }
    context
        .network
        .http_probe(&args.url, &args.method, args.timeout * 1000)
}

fn push_if_some(table: &mut Table, field: &str, value: &Option<String>) {
    if let Some(value) = value {
        table.push(row![field.to_string(), value.clone()]);
    }
}

fn verdict_text(verified: Option<bool>, detail: &Option<String>) -> String {
    let head = match verified {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    };
    match detail {
        Some(detail) => format!("{head} ({detail})"),
        None => head.to_string(),
    }
}

/// Run the chain; each stage's failure stops only the stages that depend on it.
fn run_chain(context: &SystemContext, args: &CheckArgs) -> Result<Vec<DiagStep>> {
    let mut steps = Vec::new();
    let timeout_ms = args.timeout.max(1) * 1000;

    // DNS.
    let addresses: Option<Vec<IpAddr>> = match args.host.parse::<IpAddr>() {
        Ok(address) => {
            steps.push(DiagStep::new(
                "dns",
                "ok",
                format!("literal address {address}"),
            ));
            Some(vec![address])
        }
        Err(_) => match context.network.resolve(&args.host) {
            Ok(rows) if !rows.is_empty() => {
                steps.push(DiagStep::new(
                    "dns",
                    "ok",
                    format!(
                        "{} record(s): {}",
                        rows.len(),
                        rows.iter()
                            .map(|a| a.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
                Some(rows)
            }
            Ok(_) => {
                steps.push(DiagStep::new("dns", "fail", "resolver answered no records"));
                None
            }
            Err(error) if error.kind() == ErrorKind::Unsupported => {
                steps.push(DiagStep::new(
                    "dns",
                    "warn",
                    format!("resolver unavailable: {}", error.message()),
                ));
                None
            }
            Err(error) => {
                steps.push(DiagStep::new("dns", "fail", error.message().to_string()));
                None
            }
        },
    };

    // TCP.
    let tcp_ok = match &addresses {
        Some(rows) => {
            let target = (rows[0], args.port);
            match std::net::TcpStream::connect_timeout(
                &to_sockaddr(target),
                Duration::from_millis(timeout_ms),
            ) {
                Ok(_) => {
                    steps.push(DiagStep::new(
                        "tcp",
                        "ok",
                        format!("{}:{} accepted a connection", rows[0], args.port),
                    ));
                    true
                }
                Err(error) => {
                    steps.push(DiagStep::new(
                        "tcp",
                        "fail",
                        format!("{}:{}: {error}", rows[0], args.port),
                    ));
                    false
                }
            }
        }
        None => {
            steps.push(DiagStep::new("tcp", "fail", "skipped: no address to reach"));
            false
        }
    };

    // TLS + certificate.
    let mut tls_ok = false;
    let info = if tcp_ok {
        match probe_tls(context, &args.host, args.port, args.timeout) {
            Ok(info) => {
                tls_ok = true;
                steps.push(DiagStep::new(
                    "tls",
                    "ok",
                    format!(
                        "{}{}",
                        info.protocol.clone().unwrap_or_else(|| "handshake".into()),
                        info.cipher
                            .as_ref()
                            .map(|cipher| format!(" / {cipher}"))
                            .unwrap_or_default()
                    ),
                ));
                Some(info)
            }
            Err(error) => {
                steps.push(DiagStep::new("tls", "fail", error.message().to_string()));
                None
            }
        }
    } else {
        steps.push(DiagStep::new(
            "tls",
            "fail",
            "skipped: TCP stage did not reach the port",
        ));
        None
    };
    if let Some(info) = &info {
        let detail = format!(
            "issuer {}; valid to {}{}; trust: {}",
            info.issuer.clone().unwrap_or_else(|| "?".into()),
            info.not_after.clone().unwrap_or_else(|| "?".into()),
            if info.san.is_empty() {
                String::new()
            } else {
                format!("; san {}", info.san.len())
            },
            verdict_text(info.verified, &info.verify_detail)
        );
        let verdict = match info.verified {
            Some(true) => "ok",
            Some(false) => "fail",
            None => "warn",
        };
        steps.push(DiagStep::new("cert", verdict, detail));
    } else {
        steps.push(DiagStep::new("cert", "fail", "skipped: no TLS session"));
    }

    // HTTP.
    if tls_ok {
        let url = if args.port == 443 {
            format!("https://{}", args.host)
        } else {
            format!("http://{}:{}", args.host, args.port)
        };
        match context.network.http_probe(&url, "GET", timeout_ms) {
            Ok(response) => {
                let code = response.status.unwrap_or(0);
                let verdict = if code < 400 {
                    "ok"
                } else if code < 500 {
                    "warn"
                } else {
                    "fail"
                };
                steps.push(DiagStep::new(
                    "http",
                    verdict,
                    format!("{code} in {} ms", format_ms(response.time_total_ms)),
                ));
            }
            Err(error) => {
                steps.push(DiagStep::new("http", "fail", error.message().to_string()));
            }
        }
    } else {
        steps.push(DiagStep::new(
            "http",
            "fail",
            "skipped: the TLS stage did not succeed",
        ));
    }

    Ok(steps)
}

fn format_ms(ms: Option<f64>) -> String {
    ms.map(|value| format!("{value:.0}"))
        .unwrap_or_else(|| "-".into())
}

fn to_sockaddr((address, port): (IpAddr, u16)) -> std::net::SocketAddr {
    std::net::SocketAddr::new(address, port)
}
