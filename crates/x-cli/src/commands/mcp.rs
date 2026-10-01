//! `x mcp`: an MCP (Model Context Protocol) server over stdio.
//!
//! It speaks JSON-RPC 2.0, line-delimited, so any MCP-aware agent (Claude,
//! CodeBuddy, …) can drive `x` as a tool provider. Read-only tools are always
//! on; destructive tools appear only when the server is started with
//! `--allow-destructive`, and even then each call must pass `confirm: true`
//! explicitly. Without confirmation the server returns the *plan* of what it
//! would do and stops — the same "show the plan, then ask" contract the CLI
//! and TUI already follow, so an agent can never mutate a machine by accident.

use clap::Subcommand;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use x_core::device::DeviceClass;
use x_core::error::Result;
use x_core::port::{PortListOptions, PortQuery, PortSort};
use x_core::process::{KillSignal, ProcessListOptions, ProcessSort};
use x_core::service::{ServiceAction, ServiceListOptions};
use x_core::SystemContext;

use crate::format::Renderer;

/// `x mcp` subcommands.
#[derive(Debug, Subcommand)]
pub enum McpCommand {
    /// Run the MCP server over stdio (JSON-RPC). Read-only tools are always
    /// available; destructive tools require `--allow-destructive` and an
    /// explicit `confirm: true` on every call.
    Serve {
        /// Allow destructive tools (`kill_process`, `kill_port`, `service_action`).
        #[arg(long)]
        allow_destructive: bool,
    },
}

/// Route an `x mcp` invocation. MCP owns stdout, so the renderer is never used.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &McpCommand,
) -> Result<i32> {
    let _ = renderer;
    match command {
        McpCommand::Serve { allow_destructive } => run_server(context, *allow_destructive),
    }
    Ok(0)
}

/// Read JSON-RPC requests from stdin until EOF, writing one response per line.
fn run_server(context: &SystemContext, allow_destructive: bool) {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { continue };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(response) = handle_message(&message, context, allow_destructive) {
            if writeln!(
                out,
                "{}",
                serde_json::to_string(&response).unwrap_or_default()
            )
            .is_err()
            {
                break; // stdout closed under us
            }
            let _ = out.flush();
        }
    }
}

/// One MCP tool's static description.
#[derive(Serialize)]
struct Tool {
    name: &'static str,
    description: &'static str,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

/// Handle a single JSON-RPC message. Returns `None` for notifications (which
/// take no response) and for lines we choose to ignore.
fn handle_message(
    message: &Value,
    context: &SystemContext,
    allow_destructive: bool,
) -> Option<Value> {
    let method = message.get("method").and_then(|m| m.as_str())?;
    let id = message.get("id").cloned();
    match method {
        "initialize" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "x", "version": env!("CARGO_PKG_VERSION") }
            }
        })),
        "ping" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {} })),
        "tools/list" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "tools": list_tools(allow_destructive) }
        })),
        "tools/call" => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default()));
            let tool_result = call_tool(name, &arguments, context, allow_destructive);
            let (content, is_error) = match tool_result {
                Ok(text) => (vec![json!({ "type": "text", "text": text })], false),
                Err(message) => (vec![json!({ "type": "text", "text": message })], true),
            };
            Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": content, "isError": is_error }
            }))
        }
        // Notifications carry no id and expect no reply.
        "notifications/initialized" | "initialized" => None,
        // Unknown method with an id: report it; without an id, stay silent.
        _ => id.map(|id| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") }
            })
        }),
    }
}

/// The tool catalog. Destructive tools are advertised only when enabled.
fn list_tools(allow_destructive: bool) -> Vec<Tool> {
    let mut tools = vec![
        Tool {
            name: "find_port",
            description: "Find sockets by port, owning process name, or substring. Returns listening and other sockets with their owners.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "port": { "type": "integer", "description": "Match sockets on this port number." },
                    "process": { "type": "string", "description": "Match sockets owned by processes whose name contains this." },
                    "search": { "type": "string", "description": "Case-insensitive substring across port, process and address." },
                    "listening_only": { "type": "boolean", "description": "Only listening sockets (default true)." },
                    "limit": { "type": "integer", "description": "Maximum rows to return." }
                }
            }),
        },
        Tool {
            name: "find_process",
            description: "Find processes by name, pid or owner, sorted by CPU by default. Returns the matching process records.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Case-insensitive substring across name, command line and executable." },
                    "pid": { "type": "integer", "description": "Exact process id." },
                    "user": { "type": "string", "description": "Only processes owned by this user." },
                    "sort": { "type": "string", "enum": ["cpu", "memory", "pid", "name", "start"], "description": "Sort key (default cpu)." },
                    "limit": { "type": "integer", "description": "Maximum rows to return." }
                }
            }),
        },
        Tool {
            name: "list_services",
            description: "List system services with their state. Optionally filtered by name or running-only.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Substring match on the service name." },
                    "running_only": { "type": "boolean", "description": "Only running services (default false)." },
                    "limit": { "type": "integer", "description": "Maximum rows to return." }
                }
            }),
        },
        Tool {
            name: "system_info",
            description: "Static system facts: OS, kernel, arch, hostname, CPU, memory, uptime, locale, user/shell/terminal.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "list_disks",
            description: "All mounted filesystems: device, type, media, capacity, used, available, usage percent.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "network_interfaces",
            description: "Network interfaces: status, MAC, MTU, link speed, traffic, gateway, link type.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "network_addresses",
            description: "IP addresses with prefix length and DHCP markers.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "network_routes",
            description: "Routing table entries (IPv4/IPv6).",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "network_dns",
            description: "DNS resolver configuration.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "list_devices",
            description: "Present hardware devices, optionally filtered by class (usb, bluetooth, audio, display, camera, input, network).",
            input_schema: json!({
                "type": "object",
                "properties": { "class": { "type": "string", "description": "Device class token to filter by." } }
            }),
        },
    ];
    if allow_destructive {
        tools.extend([
            Tool {
                name: "kill_process",
                description: "Terminate a process by pid. Requires confirm: true; without it returns the plan only.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "pid": { "type": "integer", "description": "Process id to terminate." },
                        "signal": { "type": "string", "enum": ["terminate", "interrupt", "kill", "quit", "hangup"], "description": "Kill signal (default terminate)." },
                        "confirm": { "type": "boolean", "description": "Must be true to actually terminate. Otherwise the plan is returned." }
                    },
                    "required": ["pid", "confirm"]
                }),
            },
            Tool {
                name: "kill_port",
                description: "Free a port by terminating the processes that hold it. Requires confirm: true; without it returns the plan only.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "port": { "type": "integer", "description": "Port number to free." },
                        "confirm": { "type": "boolean", "description": "Must be true to execute. Otherwise the plan is returned." }
                    },
                    "required": ["port", "confirm"]
                }),
            },
            Tool {
                name: "service_action",
                description: "Start/stop/restart/reload/enable/disable a service. Requires confirm: true; without it returns the plan only.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Service name." },
                        "action": { "type": "string", "enum": ["start", "stop", "restart", "reload", "enable", "disable"] },
                        "confirm": { "type": "boolean", "description": "Must be true to execute. Otherwise the plan is returned." }
                    },
                    "required": ["name", "action", "confirm"]
                }),
            },
        ]);
    }
    tools
}

/// Dispatch a tool call. `Ok(text)` is returned to the agent as text content;
/// `Err(message)` becomes an `isError` response.
fn call_tool(
    name: &str,
    args: &Value,
    context: &SystemContext,
    allow_destructive: bool,
) -> std::result::Result<String, String> {
    match name {
        "find_port" => find_port(args, context),
        "find_process" => find_process(args, context),
        "list_services" => list_services(args, context),
        "system_info" => Ok(serialize(&context.system.info().map_err(text_err)?)),
        "list_disks" => Ok(serialize(&context.disk.list().map_err(text_err)?)),
        "network_interfaces" => Ok(serialize(&context.network.interfaces().map_err(text_err)?)),
        "network_addresses" => Ok(serialize(&context.network.addresses().map_err(text_err)?)),
        "network_routes" => Ok(serialize(&context.network.routes().map_err(text_err)?)),
        "network_dns" => Ok(serialize(&context.network.dns().map_err(text_err)?)),
        "list_devices" => list_devices(args, context),
        "kill_process" => kill_process(args, context, allow_destructive),
        "kill_port" => kill_port(args, context, allow_destructive),
        "service_action" => service_action(args, context, allow_destructive),
        other => Err(format!("unknown tool: {other}")),
    }
}

fn text_err(error: x_core::error::Error) -> String {
    error.message().to_string()
}

fn serialize<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "[]".to_string())
}

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn get_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

fn find_port(args: &Value, context: &SystemContext) -> std::result::Result<String, String> {
    let listening_only = args
        .get("listening_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let mut options = PortListOptions {
        listening_only,
        ..Default::default()
    };
    if let Some(port) = get_u64(args, "port") {
        options.search = Some(port.to_string());
    }
    if let Some(process) = get_str(args, "process") {
        options.search = Some(process.to_string());
    }
    if let Some(search) = get_str(args, "search") {
        options.search = Some(search.to_string());
    }
    if let Some(limit) = get_u64(args, "limit") {
        options.limit = Some(limit as usize);
    }
    Ok(serialize(&context.port.list(&options).map_err(text_err)?))
}

fn find_process(args: &Value, context: &SystemContext) -> std::result::Result<String, String> {
    let sort = match get_str(args, "sort").unwrap_or("cpu") {
        "memory" => ProcessSort::Memory,
        "pid" => ProcessSort::Pid,
        "name" => ProcessSort::Name,
        "start" => ProcessSort::StartTime,
        _ => ProcessSort::Cpu,
    };
    let mut options = ProcessListOptions {
        sort,
        with_usage: true,
        ..Default::default()
    };
    if let Some(name) = get_str(args, "name") {
        options.search = Some(name.to_string());
    }
    if let Some(user) = get_str(args, "user") {
        options.user = Some(user.to_string());
    }
    if let Some(pid) = get_u64(args, "pid") {
        options.search = Some(pid.to_string());
    }
    if let Some(limit) = get_u64(args, "limit") {
        options.limit = Some(limit as usize);
    }
    Ok(serialize(
        &context.process.list(&options).map_err(text_err)?,
    ))
}

fn list_services(args: &Value, context: &SystemContext) -> std::result::Result<String, String> {
    let mut options = ServiceListOptions::default();
    if let Some(name) = get_str(args, "name") {
        options.search = Some(name.to_string());
    }
    if let Some(running_only) = args.get("running_only").and_then(|v| v.as_bool()) {
        options.running_only = running_only;
    }
    if let Some(limit) = get_u64(args, "limit") {
        options.limit = Some(limit as usize);
    }
    Ok(serialize(
        &context.service.list(&options).map_err(text_err)?,
    ))
}

fn list_devices(args: &Value, context: &SystemContext) -> std::result::Result<String, String> {
    let Some(manager) = &context.device else {
        return Err("device enumeration is not available on this platform".to_string());
    };
    let mut rows = manager.devices().map_err(text_err)?;
    if let Some(class) = get_str(args, "class") {
        let wanted = DeviceClass::parse(class);
        rows.retain(|device| device.class == wanted);
    }
    Ok(serialize(&rows))
}

fn parse_signal(value: Option<&Value>) -> KillSignal {
    match value.and_then(|v| v.as_str()) {
        Some("kill") => KillSignal::Kill,
        Some("interrupt") => KillSignal::Interrupt,
        Some("quit") => KillSignal::Quit,
        Some("hangup") => KillSignal::Hangup,
        _ => KillSignal::Terminate,
    }
}

fn kill_process(
    args: &Value,
    context: &SystemContext,
    allow_destructive: bool,
) -> std::result::Result<String, String> {
    if !allow_destructive {
        return Err(
            "destructive tools are disabled; restart `x mcp` with --allow-destructive".to_string(),
        );
    }
    let pid = get_u64(args, "pid").ok_or("missing required argument: pid")? as u32;
    let confirm = args
        .get("confirm")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let signal = parse_signal(args.get("signal"));
    if !confirm {
        let info = context.process.get(pid).map_err(text_err)?;
        return Ok(format!(
            "plan (not executed): terminate process {pid} ({}) — pass confirm: true to execute",
            info.name
        ));
    }
    context.process.kill(pid, signal).map_err(text_err)?;
    Ok(format!("killed process {pid}"))
}

fn kill_port(
    args: &Value,
    context: &SystemContext,
    allow_destructive: bool,
) -> std::result::Result<String, String> {
    if !allow_destructive {
        return Err(
            "destructive tools are disabled; restart `x mcp` with --allow-destructive".to_string(),
        );
    }
    let port = get_u64(args, "port").ok_or("missing required argument: port")? as u16;
    let confirm = args
        .get("confirm")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let signal = parse_signal(args.get("signal"));
    let plan = context
        .port
        .plan(&PortQuery::Port(port), PortSort::Port)
        .map_err(text_err)?;
    if plan.is_empty() {
        return Ok(format!("no process holds port {port}; nothing to do"));
    }
    if !confirm {
        return Ok(format!(
            "plan (not executed): {} — pass confirm: true to execute",
            plan_summary(&plan)
        ));
    }
    context.port.kill_plan(&plan, signal).map_err(text_err)?;
    Ok(format!("freed port {port}"))
}

fn service_action(
    args: &Value,
    context: &SystemContext,
    allow_destructive: bool,
) -> std::result::Result<String, String> {
    if !allow_destructive {
        return Err(
            "destructive tools are disabled; restart `x mcp` with --allow-destructive".to_string(),
        );
    }
    let name = get_str(args, "name")
        .ok_or("missing required argument: name")?
        .to_string();
    let action = match get_str(args, "action").ok_or("missing required argument: action")? {
        "start" => ServiceAction::Start,
        "stop" => ServiceAction::Stop,
        "restart" => ServiceAction::Restart,
        "reload" => ServiceAction::Reload,
        "enable" => ServiceAction::Enable,
        "disable" => ServiceAction::Disable,
        other => return Err(format!("unknown service action: {other}")),
    };
    let confirm = args
        .get("confirm")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !confirm {
        return Ok(format!(
            "plan (not executed): {action:?} service {name} — pass confirm: true to execute"
        ));
    }
    context.service.action(&name, action).map_err(text_err)?;
    Ok(format!("{action:?} service {name}"))
}

/// One-line human summary of a kill plan, used in the pre-confirmation message.
fn plan_summary(plan: &x_core::port::KillPlan) -> String {
    let count = plan.sockets.len();
    format!("terminate {count} socket(s) holding the port")
}

#[cfg(test)]
mod tests {
    use super::*;
    use x_core::testing::{stub_process, stub_service, stub_socket, Stubs};

    fn context() -> SystemContext {
        Stubs::new()
            .with_ports(vec![stub_socket(8080, 42, "node")])
            .with_processes(vec![stub_process(42, None, "node")])
            .with_services(vec![stub_service("sshd", 12)])
            .context()
    }

    fn call(name: &str, arguments: Value, allow: bool) -> Value {
        let message = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });
        handle_message(&message, &context(), allow).expect("tools/call replies")
    }

    #[test]
    fn tools_list_includes_core_read_only_tools() {
        let message = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        let response = handle_message(&message, &context(), false).unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"find_port"));
        assert!(names.contains(&"find_process"));
        assert!(names.contains(&"list_services"));
        // Destructive tools stay hidden until explicitly enabled.
        assert!(!names
            .iter()
            .any(|n| *n == "kill_process" || *n == "kill_port" || *n == "service_action"));
    }

    #[test]
    fn tools_list_exposes_destructive_tools_when_enabled() {
        let message = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        let response = handle_message(&message, &context(), true).unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"kill_process"));
        assert!(names.contains(&"kill_port"));
    }

    #[test]
    fn initialize_reports_tools_capability() {
        let message = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" });
        let response = handle_message(&message, &context(), false).unwrap();
        assert_eq!(response["result"]["capabilities"]["tools"], json!({}));
        assert_eq!(response["result"]["serverInfo"]["name"], "x");
    }

    #[test]
    fn find_port_returns_matching_sockets() {
        let response = call("find_port", json!({ "port": 8080 }), false);
        assert_eq!(response["result"]["isError"], false);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        let data: Value = serde_json::from_str(text).unwrap();
        assert_eq!(data.as_array().unwrap().len(), 1);
    }

    #[test]
    fn list_devices_without_manager_reports_gracefully() {
        // The test stub's device manager cannot list; the tool must return an
        // error (not panic) and surface the underlying message.
        let response = call("list_devices", json!({ "class": "usb" }), false);
        assert_eq!(response["result"]["isError"], true);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(!text.is_empty(), "error text must be populated");
    }

    #[test]
    fn destructive_tools_refuse_without_flag() {
        let response = call("kill_process", json!({ "pid": 42, "confirm": true }), false);
        assert_eq!(response["result"]["isError"], true);
    }

    #[test]
    fn destructive_tools_return_a_plan_without_confirm() {
        let response = call("kill_port", json!({ "port": 8080, "confirm": false }), true);
        assert_eq!(response["result"]["isError"], false);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("not executed"),
            "expected a plan, got: {text}"
        );
    }
}
