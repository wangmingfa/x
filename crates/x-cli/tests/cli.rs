//! End to end tests over the real argument grammar.
//!
//! Every test builds a context from stubs, runs the same `execute` the binary
//! runs, and asserts on the bytes a user or a script would see. Nothing here
//! depends on the host: the same table, the same JSON, the same refusals.

use std::io::Write;
use std::sync::{Arc, Mutex};

use clap::Parser;
use x_core::error::{ErrorKind, PermissionRequirement};
use x_core::testing::StubFailure;
use x_core::testing::{stub_process, stub_service, stub_socket, Stubs};
use x_core::{CpuUsage, MemoryUsage, SystemInfo};

use x_cli::format::{Confirmer, OutputFormat, Renderer};
use x_cli::{execute, Cli};

/// A confirmation that answers from a script and records what it was asked.
struct ScriptedConfirmer {
    answer: bool,
    asked: Arc<Mutex<Vec<String>>>,
}

impl ScriptedConfirmer {
    fn new(answer: bool) -> (Self, Arc<Mutex<Vec<String>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                answer,
                asked: Arc::clone(&asked),
            },
            asked,
        )
    }
}

impl Confirmer for ScriptedConfirmer {
    fn confirm(&mut self, question: &str) -> std::io::Result<bool> {
        self.asked
            .lock()
            .expect("test mutex")
            .push(question.to_string());
        Ok(self.answer)
    }
}

/// Stubs with one of everything, so every command has rows to render.
fn populated() -> Stubs {
    Stubs::new()
        .with_ports(vec![
            stub_socket(8080, 42, "node"),
            stub_socket(9090, 43, "python"),
        ])
        .with_processes(vec![stub_process(42, Some(1), "node")])
        .with_services(vec![stub_service("sshd", 42)])
        .with_system(
            SystemInfo {
                hostname: "testhost".into(),
                os_name: "testOS".into(),
                os_version: "1.2.3".into(),
                arch: "arm64".into(),
                uptime_seconds: 3600,
                cpu_count: 8,
                total_memory_bytes: 16 * 1024 * 1024 * 1024,
                available_memory_bytes: 12 * 1024 * 1024 * 1024,
                current_user: Some("tester".into()),
                ..Default::default()
            },
            CpuUsage {
                total_percent: 12.5,
                per_core_percent: vec![25.0],
            },
            MemoryUsage {
                total_bytes: 16 * 1024 * 1024 * 1024,
                used_bytes: 4 * 1024 * 1024 * 1024,
                available_bytes: 12 * 1024 * 1024 * 1024,
                percent: 25.0,
            },
        )
}

/// A `Write` sink the tests can read back after the renderer took ownership.
#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("test mutex").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buffer {
    fn text(&self) -> String {
        let bytes = self.0.lock().expect("test mutex").clone();
        String::from_utf8(bytes).expect("utf-8 output")
    }
}

struct Output {
    stdout: String,
    stderr: String,
    code: i32,
}

/// Run a command line, capturing both streams.
fn x(stubs: &Stubs, args: &[&str], answer: bool) -> Output {
    let cli = Cli::parse_from(std::iter::once("x").chain(args.iter().copied()));
    let format = if cli.json {
        OutputFormat::Json
    } else if cli.plain {
        OutputFormat::Plain
    } else {
        OutputFormat::Table
    };

    let stdout = Buffer::default();
    let mut renderer = Renderer::to_sink(format, false, stdout.clone());
    let (mut confirmer, _asked) = ScriptedConfirmer::new(answer);

    let outcome = execute(&stubs.context(), &cli, &mut renderer, &mut confirmer);
    let flush = renderer.flush();
    let stderr = Buffer::default();

    let code = match (outcome, flush) {
        (Ok(code), Ok(())) => code,
        (Ok(_), Err(_)) => 1,
        (Err(error), _) => {
            let mut text = format!("error: {}", error.message());
            if let Some(permission) = error.permission() {
                text.push_str(&format!("\nhint: {}", permission.guidance()));
            }
            let mut sink = stderr.clone();
            sink.write_all(text.as_bytes()).ok();
            error.exit_code()
        }
    };

    Output {
        stdout: stdout.text(),
        stderr: stderr.text(),
        code,
    }
}

#[test]
fn no_arguments_lists_listening_sockets() {
    let stubs = populated();
    let out = x(&stubs, &[], false);

    assert_eq!(out.code, 0);
    assert!(
        out.stdout.contains("port"),
        "missing header: {}",
        out.stdout
    );
    assert!(out.stdout.contains("8080"));
    assert!(out.stdout.contains("node"));
}

#[test]
fn json_is_valid_and_stable() {
    let stubs = populated();
    let out = x(&stubs, &["--json", "port", "list"], false);

    assert_eq!(out.code, 0);
    let start = out.stdout.find('[').expect("json array");
    let value: serde_json::Value = serde_json::from_str(&out.stdout[start..]).expect("valid json");
    let rows = value.as_array().expect("array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["local_port"], 8080);
    assert_eq!(rows[0]["process_name"], "node");
    assert_eq!(rows[0]["state"], "listen");
    assert!(
        !out.stdout[..start].contains('{'),
        "nothing may precede the JSON"
    );
}

#[test]
fn plain_output_is_one_tab_separated_record_per_line() {
    let stubs = populated();
    let out = x(&stubs, &["--plain", "port", "list"], false);

    let lines: Vec<&str> = out.stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0].split('\t').count(),
        7,
        "column count: {:?}",
        lines[0]
    );
    assert!(lines[0].starts_with("8080"));
}

#[test]
fn limit_is_honoured_by_every_listing() {
    let stubs = populated();

    let ports = x(&stubs, &["--plain", "port", "list", "--limit", "1"], false);
    assert_eq!(ports.stdout.lines().count(), 1);

    let processes = x(&stubs, &["--plain", "ps", "list", "--limit", "1"], false);
    assert_eq!(processes.stdout.lines().count(), 1);
}

#[test]
fn port_check_reports_the_port_it_was_asked_about() {
    let stubs = populated();

    let listening = x(&stubs, &["port", "check", "8080"], false);
    assert_eq!(
        listening.code, 0,
        "something holds 8080: that is the answer"
    );
    assert!(listening.stdout.contains("in use"));

    let free = x(&stubs, &["port", "check", "12345"], false);
    assert_eq!(free.code, 1, "a free port is information, not an error");
    assert!(free.stdout.contains("free"));

    let json = x(&stubs, &["--json", "port", "check", "8080"], false);
    let value: serde_json::Value = serde_json::from_str(&json.stdout).expect("valid json");
    assert_eq!(value["in_use"], true);
    assert_eq!(value["port"], 8080);
    assert_eq!(value["owners"][0]["local_port"], 8080);
}

#[test]
fn killing_without_yes_asks_first_and_does_nothing_when_refused() {
    let stubs = populated();

    let refused = x(&stubs, &["port", "kill", "8080"], false);
    assert_eq!(refused.code, 130, "declining is not a success");
    assert!(refused.stdout.contains("aborted"), "{}", refused.stdout);
    assert!(stubs.port.killed().is_empty());
}

#[test]
fn a_destructive_json_run_emits_exactly_one_document() {
    let stubs = populated();

    let out = x(&stubs, &["--json", "port", "kill", "8080", "--yes"], true);
    let value: serde_json::Value = serde_json::from_str(&out.stdout).expect("one json document");
    assert_eq!(value["killed"], 1);
    assert_eq!(value["plan"]["sockets"][0]["local_port"], 8080);

    let processes = x(&stubs, &["--json", "ps", "kill", "42", "--yes"], true);
    let value: serde_json::Value = serde_json::from_str(&processes.stdout).expect("one document");
    assert_eq!(value["killed"], 1);
    assert_eq!(value["victims"][0]["pid"], 42);

    let nothing = x(&stubs, &["--json", "port", "kill", "12345", "--yes"], true);
    assert_eq!(nothing.code, 3);
    let value: serde_json::Value = serde_json::from_str(&nothing.stdout).expect("one document");
    assert_eq!(value["killed"], serde_json::Value::Null);
    assert_eq!(value["plan"]["sockets"].as_array().map(Vec::len), Some(0));
}

#[test]
fn yes_skips_the_prompt_and_kills_the_owner() {
    let stubs = populated();

    let out = x(&stubs, &["port", "kill", "8080", "--yes"], true);
    assert_eq!(out.code, 0);
    assert_eq!(stubs.port.killed(), vec![42]);
}

#[test]
fn killing_an_unknown_port_is_not_found_not_success() {
    let stubs = populated();

    let out = x(&stubs, &["port", "kill", "12345", "--yes"], true);
    assert_eq!(
        out.code, 3,
        "nothing matched: a distinct exit code, not success"
    );
    assert!(out.stdout.contains("12345"), "{}", out.stdout);
    assert!(stubs.port.killed().is_empty());
}

#[test]
fn process_kill_by_pid_and_by_name() {
    let stubs = populated();

    assert_eq!(x(&stubs, &["ps", "kill", "42", "--yes"], true).code, 0);
    assert_eq!(stubs.process.killed(), vec![42]);

    assert_eq!(
        x(&stubs, &["ps", "kill-by-name", "node", "--yes"], true).code,
        0
    );
    assert_eq!(stubs.process.killed(), vec![42, 42]);
}

#[test]
fn service_actions_go_through_the_manager() {
    let stubs = populated();

    let out = x(&stubs, &["service", "restart", "sshd", "--yes"], true);
    assert_eq!(out.code, 0);
    assert_eq!(
        stubs.service.actions(),
        vec![("sshd".to_string(), x_core::service::ServiceAction::Restart)]
    );
}

#[test]
fn system_commands_render_the_stub_facts() {
    let stubs = populated();

    let info = x(&stubs, &["sys", "info"], false);
    assert_eq!(info.code, 0);
    assert!(info.stdout.contains("testhost"));
    assert!(info.stdout.contains("testOS"));

    let cpu = x(&stubs, &["--json", "sys", "cpu"], false);
    assert!(cpu.stdout.contains("12.5"));
}

#[test]
fn empty_results_are_a_success_not_an_error() {
    let stubs = Stubs::new();

    for args in [
        &["port", "list"][..],
        &["ps", "list"][..],
        &["service", "list"][..],
        &["disk", "list"][..],
        &["net", "dns"][..],
    ] {
        let out = x(&stubs, args, false);
        assert_eq!(out.code, 0, "{args:?} -> {}", out.stderr);
    }
}

#[test]
fn permission_errors_explain_themselves() {
    let stubs = Stubs::new();
    stubs.port.fail_with(StubFailure::denied(
        PermissionRequirement::Root,
        "operation not permitted",
    ));

    let out = x(&stubs, &["port", "list"], false);
    assert_eq!(out.code, ErrorKind::PermissionDenied.exit_code());
    assert!(out.stdout.is_empty());
    assert!(out.stderr.contains("operation not permitted"));
    assert!(out.stderr.contains("hint:"), "no guidance offered");
}

#[test]
fn version_info_needs_no_data_at_all() {
    let stubs = Stubs::new();
    stubs
        .port
        .fail_with(StubFailure::new(ErrorKind::System, "must not be called"));

    let out = x(&stubs, &["--version-info"], false);
    assert_eq!(out.code, 0);
    assert!(out.stdout.to_lowercase().contains("contract"));
}
