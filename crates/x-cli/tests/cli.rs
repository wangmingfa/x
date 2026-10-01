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
use x_core::testing::{stub_context, stub_process, stub_service, stub_socket, Stubs};
use x_core::{CpuUsage, LoadAverage, MemoryUsage, SystemContext, SystemInfo};

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
                boot_time: Some(1_735_689_600),
                timezone: Some("Asia/Shanghai".into()),
                utc_offset_seconds: Some(8 * 3_600),
                locale: Some("zh_CN.UTF-8".into()),
                current_shell: Some("zsh".into()),
                terminal: Some("iTerm.app".into()),
                performance_cores: Some(6),
                efficiency_cores: Some(2),
                ..Default::default()
            },
            CpuUsage {
                total_percent: 12.5,
                per_core_percent: vec![25.0, 4.0],
                load_average: Some(LoadAverage {
                    one: 1.2,
                    five: 0.9,
                    fifteen: 0.5,
                }),
                frequency_mhz: Some(2400.0),
                max_frequency_mhz: Some(3200.0),
                per_core_frequency_mhz: vec![2400.0, 1200.0],
                temperature_celsius: Some(54.5),
                governor: Some("schedutil".into()),
            },
            MemoryUsage {
                total_bytes: 16 * 1024 * 1024 * 1024,
                used_bytes: 4 * 1024 * 1024 * 1024,
                available_bytes: 12 * 1024 * 1024 * 1024,
                percent: 25.0,
                swap_total_bytes: 0,
                swap_used_bytes: 0,
                pressure: None,
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

/// Run a command line against a hand-built context.
fn x_in(context: &SystemContext, args: &[&str], answer: bool) -> Output {
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

    let outcome = execute(context, &cli, &mut renderer, &mut confirmer);
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

/// Run a command line against the context assembled by `stubs`.
fn x(stubs: &Stubs, args: &[&str], answer: bool) -> Output {
    x_in(&stubs.context(), args, answer)
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
        8,
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
fn port_find_lists_sockets_of_matching_process() {
    let stubs = populated();

    let found = x(&stubs, &["port", "find", "node"], false);
    assert_eq!(found.code, 0);
    assert!(found.stdout.contains("8080"));
    assert!(!found.stdout.contains("9090"));

    let missing = x(&stubs, &["port", "find", "ghost"], false);
    assert_eq!(
        missing.code, 3,
        "no match is a not-found, like kill previews"
    );
    assert!(missing.stdout.contains("no sockets"));
}

#[test]
fn port_watch_prints_the_baseline_and_nothing_when_nothing_changed() {
    let stubs = populated();
    let out = x(
        &stubs,
        &["port", "watch", "--interval", "0.05", "--count", "2"],
        false,
    );
    assert_eq!(out.code, 0);
    assert!(
        out.stdout.contains("watching 2 socket(s)"),
        "baseline missing: {}",
        out.stdout
    );
    assert_eq!(
        out.stdout.lines().count(),
        1,
        "static stubs must not produce diff rows: {:?}",
        out.stdout
    );
}

#[test]
fn socket_rows_show_remote_endpoints() {
    let mut established = stub_socket(443, 42, "node");
    established.state = x_core::ConnectionState::Established;
    established.remote_address = Some("93.184.216.34".parse().unwrap());
    established.remote_port = Some(51234);
    let stubs = Stubs::new().with_ports(vec![established]);

    let out = x(&stubs, &["--plain", "port", "all"], false);
    assert_eq!(out.code, 0);
    assert!(
        out.stdout.contains("93.184.216.34:51234"),
        "remote column missing: {:?}",
        out.stdout
    );
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
fn service_logs_renders_the_page_from_the_manager() {
    let stubs = populated();
    stubs.service.set_logs(x_core::service::ServiceLogPage {
        service: "sshd".into(),
        source: "journalctl".into(),
        entries: vec![
            x_core::service::ServiceLogEntry {
                timestamp: Some("Oct 05 10:23:41".into()),
                level: Some("error".into()),
                message: "bad auth".into(),
            },
            x_core::service::ServiceLogEntry {
                timestamp: None,
                level: None,
                message: "plain line".into(),
            },
        ],
    });

    let out = x(&stubs, &["service", "logs", "sshd", "--lines", "2"], true);
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("bad auth"), "missing: {}", out.stdout);
    assert!(out.stdout.contains("plain line"));
    assert!(out.stdout.contains("timestamp"));

    let json = x(&stubs, &["--json", "service", "logs", "sshd"], true);
    assert_eq!(json.code, 0);
    let start = json.stdout.find('{').expect("json object");
    let value: serde_json::Value = serde_json::from_str(&json.stdout[start..]).expect("valid json");
    assert_eq!(value["service"], "sshd");
    assert_eq!(value["entries"][0]["level"], "error");
    assert!(value["entries"][1].get("level").is_none());
}

#[test]
fn service_logs_without_a_log_source_is_unsupported() {
    let stubs = populated();
    let out = x(&stubs, &["service", "logs", "sshd"], true);
    assert_eq!(out.code, ErrorKind::Unsupported.exit_code());
}

#[test]
fn service_native_passes_arguments_through() {
    let stubs = populated();
    let out = x(&stubs, &["service", "native", "query", "sshd"], true);
    assert_eq!(out.code, 0);
    assert_eq!(
        stubs.service.native_calls(),
        vec![vec!["query".to_string(), "sshd".to_string()]]
    );
    assert!(out.stdout.contains("$ stub query sshd"));
}

#[test]
fn capability_report_covers_every_domain() {
    let stubs = populated();
    let out = x(&stubs, &["capability"], false);
    assert_eq!(out.code, 0);
    for domain in ["system", "process", "port", "net", "disk", "service"] {
        assert!(
            out.stdout.contains(domain),
            "missing {domain}: {}",
            out.stdout
        );
    }
    assert!(out.stdout.contains("service logs"));
    assert!(out.stdout.contains("status"));
}

#[test]
fn capability_domain_filter_narrows_the_report() {
    let stubs = populated();
    let out = x(&stubs, &["capability", "--domain", "service"], false);
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("service manager"));
    assert!(
        !out.stdout.contains("socket list"),
        "filter leaked other domains: {}",
        out.stdout
    );

    let empty = x(&stubs, &["capability", "--domain", "nonsense"], false);
    assert_eq!(empty.code, 0);
    assert!(empty.stdout.contains("no capability rows"));
}

#[test]
fn capability_json_is_a_row_array_with_snake_statuses() {
    let stubs = populated();
    let out = x(
        &stubs,
        &["--json", "capability", "--domain", "service"],
        false,
    );
    assert_eq!(out.code, 0);
    let start = out.stdout.find('[').expect("json array");
    let value: serde_json::Value = serde_json::from_str(&out.stdout[start..]).expect("valid json");
    let rows = value.as_array().expect("array");
    assert!(rows.iter().all(|row| row["domain"] == "service"));
    let logs = rows
        .iter()
        .find(|row| row["feature"] == "service logs")
        .expect("logs row");
    // The default stub answers `logs` with Unsupported.
    assert_eq!(logs["status"], "unsupported");
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
fn sys_info_renders_the_completion_fields() {
    let stubs = populated();

    let table = x(&stubs, &["sys", "info"], false);
    // The boot timestamp is rendered in the reported offset, not as a raw epoch.
    assert!(
        table.stdout.contains("2025-01-01 08:00:00 +08:00"),
        "{}",
        table.stdout
    );
    for field in ["timezone", "locale", "user", "shell", "terminal"] {
        assert!(table.stdout.contains(field), "{field} missing");
    }
    assert!(table.stdout.contains("zh_CN.UTF-8"));
    assert!(table.stdout.contains("iTerm.app"));

    let json = x(&stubs, &["--json", "sys", "info"], false);
    assert_eq!(json.code, 0);
    let value: serde_json::Value = serde_json::from_str(&json.stdout).expect("json");
    assert_eq!(value["timezone"], "Asia/Shanghai");
    assert_eq!(value["boot_time"].as_u64(), Some(1_735_689_600));
    assert_eq!(value["utc_offset_seconds"].as_i64(), Some(28_800));
    assert_eq!(value["current_shell"], "zsh");
}

#[test]
fn sys_info_omits_facts_the_platform_does_not_expose() {
    let stubs = Stubs::new();
    let out = x(&stubs, &["sys", "info"], false);
    assert_eq!(out.code, 0);
    // A stub with no boot time must not print a fabricated timestamp row.
    assert!(!out.stdout.contains("last reboot"), "{}", out.stdout);
    assert!(!out.stdout.contains("timezone"), "{}", out.stdout);
    assert!(!out.stdout.contains("performance cores"), "{}", out.stdout);
}

#[test]
fn sys_cpu_renders_the_detail_the_platform_reports() {
    let stubs = populated();

    let table = x(&stubs, &["sys", "cpu"], false);
    assert_eq!(table.code, 0);
    assert!(
        table.stdout.contains("load 1.20 0.90 0.50"),
        "{}",
        table.stdout
    );
    assert!(
        table.stdout.contains("clock 2400 MHz of 3200 MHz"),
        "{}",
        table.stdout
    );
    assert!(
        table.stdout.contains("temp 54.5 C") && table.stdout.contains("governor schedutil"),
        "{}",
        table.stdout
    );
    // Per core clocks get their own column, one row per core.
    assert!(table.stdout.contains("mhz"), "{}", table.stdout);
    assert!(table.stdout.contains("2400"));
    assert!(table.stdout.contains("1200"));

    let json = x(&stubs, &["--json", "sys", "cpu"], false);
    let value: serde_json::Value = serde_json::from_str(&json.stdout).expect("json");
    assert_eq!(value["load_average"]["one"].as_f64(), Some(1.2));
    assert_eq!(value["max_frequency_mhz"].as_f64(), Some(3200.0));
    assert_eq!(value["governor"], "schedutil");
    assert_eq!(value["per_core_frequency_mhz"].as_array().unwrap().len(), 2);
}

#[test]
fn sys_cpu_stays_silent_about_what_the_platform_cannot_see() {
    let stubs = Stubs::new();

    let table = x(&stubs, &["sys", "cpu"], false);
    assert_eq!(table.code, 0);
    for detail in ["load ", "clock ", "temp ", "governor", "mhz"] {
        assert!(
            !table.stdout.contains(detail),
            "{detail} in {}",
            table.stdout
        );
    }

    // An absent maximum must not be invented, and `temperature_celsius` for a
    // platform without a readable sensor stays out of the JSON.
    let json = x(&stubs, &["--json", "sys", "cpu"], false);
    let value: serde_json::Value = serde_json::from_str(&json.stdout).expect("json");
    assert!(value.get("max_frequency_mhz").is_none(), "{value}");
    assert!(value.get("temperature_celsius").is_none(), "{value}");
    assert!(value.get("load_average").is_none(), "{value}");
}

#[test]
fn sys_info_renders_the_core_split() {
    let stubs = populated();

    let table = x(&stubs, &["sys", "info"], false);
    assert!(
        table.stdout.contains("performance cores"),
        "{}",
        table.stdout
    );
    assert!(table.stdout.contains("efficiency cores"));

    let json = x(&stubs, &["--json", "sys", "info"], false);
    let value: serde_json::Value = serde_json::from_str(&json.stdout).expect("json");
    assert_eq!(value["performance_cores"].as_u64(), Some(6));
    assert_eq!(value["efficiency_cores"].as_u64(), Some(2));
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

/// One firewall rule for the tests below.
fn fw_rule(name: &str, action: &str, port: &str, proto: &str) -> x_core::firewall::FirewallRule {
    x_core::firewall::FirewallRule {
        name: name.into(),
        action: action.into(),
        port: Some(port.into()),
        protocol: Some(proto.into()),
        enabled: true,
    }
}

#[test]
fn firewall_status_reports_stack_and_state() {
    let stubs = Stubs::new().with_firewall(true, Vec::new());

    let out = x(&stubs, &["firewall", "status"], false);
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("stack"), "{}", out.stdout);
    assert!(out.stdout.contains("yes"), "{}", out.stdout);

    let json = x(&stubs, &["--json", "firewall", "status"], false);
    let value: serde_json::Value = serde_json::from_str(&json.stdout).expect("valid json");
    assert_eq!(value["stack"], "unknown");
    assert_eq!(value["enabled"], true);
}

#[test]
fn firewall_list_honours_the_limit_and_says_what_it_hid() {
    let stubs = Stubs::new().with_firewall(
        true,
        vec![
            fw_rule("web", "allow", "80", "tcp"),
            fw_rule("dns", "allow", "53", "udp"),
            fw_rule("backdoor", "block", "8080", "tcp"),
        ],
    );

    let all = x(&stubs, &["--plain", "firewall", "list"], false);
    assert_eq!(all.stdout.lines().count(), 3);

    let capped = x(
        &stubs,
        &["--plain", "firewall", "list", "--limit", "2"],
        false,
    );
    assert!(capped.stdout.contains("web"));
    assert!(
        !capped.stdout.contains("backdoor"),
        "the limit must actually cut: {}",
        capped.stdout
    );
    assert!(
        capped.stdout.contains("1 more rule(s)"),
        "hidden rows are announced: {}",
        capped.stdout
    );
}

#[test]
fn firewall_write_asks_first_and_changes_nothing_when_refused() {
    let stubs = Stubs::new().with_firewall(true, Vec::new());

    let refused = x(&stubs, &["firewall", "allow", "8080"], false);
    assert_eq!(refused.code, ErrorKind::InvalidInput.exit_code());
    assert!(refused.stderr.contains("aborted"));
    assert!(stubs.firewall.changes().is_empty());
}

#[test]
fn firewall_writes_reach_the_manager_when_confirmed() {
    let stubs = Stubs::new().with_firewall(true, Vec::new());

    let out = x(&stubs, &["firewall", "allow", "8080", "--yes"], true);
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("allowed inbound tcp port 8080"));
    assert_eq!(
        stubs.firewall.changes(),
        vec![("allow".to_string(), 8080u16, "tcp".to_string())]
    );

    let blocked = x(
        &stubs,
        &["firewall", "deny", "53", "--proto", "udp", "--yes"],
        true,
    );
    assert_eq!(blocked.code, 0);
    assert_eq!(
        stubs.firewall.changes()[1],
        ("deny".to_string(), 53u16, "udp".to_string())
    );
}

#[test]
fn firewall_json_write_skips_the_prompt() {
    let stubs = Stubs::new().with_firewall(true, Vec::new());

    let out = x(
        &stubs,
        &["--json", "firewall", "allow", "443", "--yes"],
        true,
    );
    assert_eq!(out.code, 0);
    assert!(
        !out.stdout.contains("about to"),
        "no prompt text in a json run: {}",
        out.stdout
    );
    assert_eq!(
        stubs.firewall.changes(),
        vec![("allow".to_string(), 443u16, "tcp".to_string())]
    );
}

#[test]
fn firewall_without_the_capability_is_unsupported() {
    let out = x_in(&stub_context(), &["firewall", "status"], false);
    assert_eq!(out.code, ErrorKind::Unsupported.exit_code());
}
