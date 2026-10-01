//! `x remote`: talk to another machine that runs x.
//!
//! Two halves, deliberately different in mechanism:
//!
//! - `server` exposes *read-only* snapshots of this machine as JSON lines
//!   over TCP. Killing, service control and everything destructive is
//!   refused by the protocol itself, not by a flag — a remote viewer can
//!   look, never touch.
//! - `connect` is the opposite pole: nothing is spoken in x's own protocol,
//!   it just runs `ssh <host> -- x <args…>` with stdio inherited, so whatever
//!   the remote x prints (including its TUI) arrives locally unchanged.

use clap::Subcommand;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Arc;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::format::{OutputFormat, Renderer};

/// `x remote` subcommands.
#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    /// Serve read-only snapshots over TCP (JSON lines).
    Server {
        /// TCP port to listen on.
        #[arg(long, default_value_t = 4188)]
        port: u16,
        /// Bind address (`127.0.0.1` by default; `0.0.0.0` exposes to LAN).
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
    },

    /// Run x on a remote host over SSH and pass the output through.
    Connect {
        /// SSH destination (alias from `~/.ssh/config`, or user@host).
        host: String,
        /// The command to run remotely, without the leading `x`
        /// (e.g. `port list --json`); defaults to the remote TUI.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// Arguments for `x remote`.
#[derive(Debug, clap::Args)]
pub struct RemoteArgs {
    #[command(subcommand)]
    pub command: RemoteCommand,
}

/// Route a `x remote` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &RemoteCommand,
) -> Result<i32> {
    match command {
        RemoteCommand::Server { port, bind } => serve(context, renderer, *port, bind),
        RemoteCommand::Connect { host, args } => connect(renderer, host, args),
    }
}

fn connect(renderer: &mut Renderer, host: &str, args: &[String]) -> Result<i32> {
    let remote_command = if args.is_empty() {
        // Default: the remote TUI over the same SSH channel.
        vec!["x".to_string()]
    } else {
        let mut words = vec!["x".to_string()];
        words.extend(args.iter().cloned());
        words
    };
    let status = std::process::Command::new("ssh")
        .arg(host)
        .arg("--")
        .args(&remote_command)
        .status()
        .map_err(|e| Error::unsupported(format!("ssh is not available: {e}")))?;
    let _ = renderer;
    Ok(status.code().unwrap_or(1))
}

/// One request line: `{"id":1,"action":"ports","params":{}}`.
/// One response line: `{"id":1,"ok":true,"data":…}` or `{"id":1,"ok":false,"error":"…"}`.
fn serve(context: &SystemContext, renderer: &mut Renderer, port: u16, bind: &str) -> Result<i32> {
    let listener = TcpListener::bind((bind, port))
        .map_err(|e| Error::system(format!("cannot listen on {bind}:{port}: {e}")))?;
    renderer.line(format!(
        "remote server listening on {bind}:{port} (read-only; destructive actions are refused)"
    ))?;
    renderer.line("actions: system | ports | processes | services | disks — Ctrl-C to stop")?;

    let context = Arc::new(ShallowSnapshot {
        system: context.system.clone(),
        port: context.port.clone(),
        process: context.process.clone(),
        service: context.service.clone(),
        disk: context.disk.clone(),
    });
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let context = context.clone();
        std::thread::spawn(move || {
            let mut writer = match stream.try_clone() {
                Ok(writer) => writer,
                Err(_) => return,
            };
            let reader = BufReader::new(stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let response = handle_line(&context, &line);
                if writer
                    .write_all(response.as_bytes())
                    .and_then(|_| writer.write_all(b"\n"))
                    .is_err()
                {
                    break;
                }
            }
        });
    }
    Ok(0)
}

/// The read-only slice of the context the server is allowed to touch.
struct ShallowSnapshot {
    system: Arc<dyn x_core::system::SystemManager>,
    port: Arc<dyn x_core::port::PortManager>,
    process: Arc<dyn x_core::process::ProcessManager>,
    service: Arc<dyn x_core::service::ServiceManager>,
    disk: Arc<dyn x_core::disk::DiskManager>,
}

/// Turn one request line into one response line. Never panics: a bad request
/// is an `ok:false` reply, not a dead connection.
fn handle_line(snapshot: &ShallowSnapshot, line: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return error_response(None, "request must be a JSON object per line");
    };
    let id = value.get("id").cloned();
    let Some(action) = value.get("action").and_then(|a| a.as_str()) else {
        return error_response(id, "missing `action`");
    };
    let data = match action {
        "system" => snapshot
            .system
            .info()
            .map(|info| serde_json::to_value(info).unwrap_or_default()),
        "ports" => snapshot
            .port
            .list(&Default::default())
            .map(|rows| serde_json::to_value(rows).unwrap_or_default()),
        "processes" => snapshot
            .process
            .list(&Default::default())
            .map(|rows| serde_json::to_value(rows).unwrap_or_default()),
        "services" => snapshot
            .service
            .list(&Default::default())
            .map(|rows| serde_json::to_value(rows).unwrap_or_default()),
        "disks" => snapshot
            .disk
            .list()
            .map(|rows| serde_json::to_value(rows).unwrap_or_default()),
        other => Err(Error::unsupported(format!(
            "action `{other}` does not exist (read-only server: system, ports, processes, services, disks)"
        ))),
    };
    match data {
        Ok(data) => serde_json::json!({ "id": id, "ok": true, "data": data }).to_string(),
        Err(error) => error_response(id, error.message()),
    }
}

fn error_response(id: Option<serde_json::Value>, message: &str) -> String {
    serde_json::json!({ "id": id, "ok": false, "error": message }).to_string()
}

/// Whether the remote server output should be silenced (JSON callers).
#[allow(dead_code)]
fn json_mode(renderer: &Renderer) -> bool {
    renderer.format() == OutputFormat::Json
}

#[cfg(test)]
mod tests {
    use super::*;
    use x_core::testing::{stub_socket, Stubs};

    /// A read-only snapshot over stub managers holding one listening socket.
    fn snapshot_with_ports() -> ShallowSnapshot {
        let stubs = Stubs::new().with_ports(vec![stub_socket(8080, 42, "node")]);
        let ctx = stubs.context();
        ShallowSnapshot {
            system: ctx.system.clone(),
            port: ctx.port.clone(),
            process: ctx.process.clone(),
            service: ctx.service.clone(),
            disk: ctx.disk.clone(),
        }
    }

    fn parse(line: &str) -> serde_json::Value {
        serde_json::from_str(line).expect("handle_line always returns JSON")
    }

    #[test]
    fn bad_json_is_an_error_response_not_a_panic() {
        let snapshot = snapshot_with_ports();
        let response = parse(&handle_line(&snapshot, "not json at all"));
        assert_eq!(response["ok"], false);
        assert!(response["error"].as_str().unwrap().contains("JSON"));
    }

    #[test]
    fn missing_action_is_rejected() {
        let snapshot = snapshot_with_ports();
        let response = parse(&handle_line(&snapshot, r#"{"id":1}"#));
        assert_eq!(response["ok"], false);
        assert!(response["error"].as_str().unwrap().contains("action"));
    }

    #[test]
    fn ports_action_returns_ok_with_data() {
        let snapshot = snapshot_with_ports();
        let response = parse(&handle_line(&snapshot, r#"{"id":2,"action":"ports"}"#));
        assert_eq!(response["ok"], true);
        let data = response["data"].as_array().expect("ports data is an array");
        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["local_port"], 8080);
    }

    #[test]
    fn unknown_action_lists_the_read_only_surface() {
        // A destructive action must never exist on the read-only server; the
        // error names the actions that do.
        let snapshot = snapshot_with_ports();
        let response = parse(&handle_line(&snapshot, r#"{"id":3,"action":"kill"}"#));
        assert_eq!(response["ok"], false);
        let message = response["error"].as_str().unwrap();
        assert!(message.contains("read-only") && message.contains("ports"));
    }

    #[test]
    fn id_round_trips_in_the_response() {
        let snapshot = snapshot_with_ports();
        let response = parse(&handle_line(
            &snapshot,
            r#"{"id":"abc","action":"services"}"#,
        ));
        assert_eq!(response["id"], "abc");
    }
}
