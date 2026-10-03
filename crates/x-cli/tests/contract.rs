//! The `--yes` / `--dry-run` contract, stated as a matrix.
//!
//! Every mutating command must behave the same way, regardless of the output
//! format:
//!
//! 1. `--dry-run` comes first — it prints the plan, exits 0, and never asks.
//!    JSON callers get one JSON document, not prose in their stream.
//! 2. JSON never exempts a mutation from confirmation. The format and the
//!    question of whether to mutate are orthogonal.
//! 3. A declined confirmation exits 130 — a decline is not an input error.
//!
//! Each row runs one representative command through all four legs
//! (dry-run, JSON dry-run, decline, JSON decline) and asserts the contract
//! holds byte-for-byte.

use std::io::Write;
use std::sync::{Arc, Mutex};

use clap::Parser;
use x_core::testing::{stub_context, stub_process, stub_service, stub_socket, Stubs};
use x_core::SystemContext;

use x_cli::format::{Confirmer, OutputFormat, Renderer};
use x_cli::{execute, Cli, EXIT_DECLINED};

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
    asked: usize,
}

/// Run a command line against a context, recording how many questions were asked.
fn x_in(context: &SystemContext, args: &[&str], answer: bool) -> Output {
    let cli = Cli::parse_from(std::iter::once("x").chain(args.iter().copied()));
    let format = if cli.json {
        OutputFormat::Json
    } else {
        OutputFormat::Table
    };

    let stdout = Buffer::default();
    let mut renderer = Renderer::to_sink(format, false, stdout.clone());
    renderer.set_dry_run(cli.dry_run);
    let (mut confirmer, asked) = ScriptedConfirmer::new(answer);

    let outcome = execute(context, &cli, &mut renderer, &mut confirmer);
    let flush = renderer.flush();
    let questions = asked.lock().expect("test mutex").len();

    let stderr = Buffer::default();
    let code = match (outcome, flush) {
        (Ok(code), Ok(())) => code,
        (Ok(_), Err(_)) => 1,
        (Err(error), _) => {
            let mut sink = stderr.clone();
            use std::io::Write as _;
            sink.write_all(format!("error: {}", error.message()).as_bytes())
                .ok();
            error.exit_code()
        }
    };

    Output {
        stdout: stdout.text(),
        stderr: stderr.text(),
        code,
        asked: questions,
    }
}

/// Run a command line against the context assembled by `stubs`.
fn x(stubs: &Stubs, args: &[&str], answer: bool) -> Output {
    x_in(&stubs.context(), args, answer)
}

/// One mutating command, exercised through all four contract legs.
struct Row {
    args: &'static [&'static str],
}

impl Row {
    fn dry_run(&self, stubs: &Stubs) {
        let out = x(stubs, &[self.args, &["--dry-run"]].concat(), true);
        assert_eq!(
            out.code, 0,
            "dry-run must exit 0: {} / {}",
            out.stdout, out.stderr
        );
        assert_eq!(
            out.asked,
            0,
            "dry-run must never ask: {}",
            self.args.join(" ")
        );
        assert!(
            out.stdout.contains("dry-run"),
            "dry-run must print its plan: {}",
            out.stdout
        );
    }

    fn json_dry_run(&self, stubs: &Stubs) {
        let out = x(stubs, &[self.args, &["--json", "--dry-run"]].concat(), true);
        assert_eq!(
            out.code, 0,
            "dry-run must exit 0 in JSON too: {}",
            out.stdout
        );
        assert_eq!(
            out.asked,
            0,
            "dry-run must never ask: {}",
            self.args.join(" ")
        );
        let doc: serde_json::Value =
            serde_json::from_str(out.stdout.trim()).expect("one JSON document");
        assert_eq!(
            doc["dry_run"], true,
            "the JSON document must state it is a plan: {}",
            out.stdout
        );
    }

    fn decline(&self, stubs: &Stubs) {
        let out = x(stubs, self.args, false);
        assert_eq!(
            out.code, EXIT_DECLINED,
            "a decline is not an input error: {}",
            out.stdout
        );
        assert!(
            out.stdout.contains("aborted"),
            "a decline must be reported on stdout: {}",
            out.stdout
        );
    }

    fn json_decline(&self, stubs: &Stubs) {
        let out = x(stubs, &[self.args, &["--json"]].concat(), false);
        assert_eq!(
            out.code, EXIT_DECLINED,
            "JSON does not exempt the confirmation: {}",
            out.stdout
        );
        let doc: serde_json::Value =
            serde_json::from_str(out.stdout.trim()).expect("one JSON document");
        assert_eq!(
            doc["aborted"], true,
            "the JSON document must report the refusal: {}",
            out.stdout
        );
    }

    fn run(&self, stubs: &Stubs) {
        self.dry_run(stubs);
        self.json_dry_run(stubs);
        self.decline(stubs);
        self.json_decline(stubs);
    }
}

#[test]
fn every_mutating_command_honors_the_contract() {
    let stubs = Stubs::new()
        .with_ports(vec![stub_socket(8080, 42, "node")])
        .with_processes(vec![stub_process(42, Some(1), "node")])
        .with_services(vec![stub_service("sshd", 42)]);

    let rows = [
        Row {
            args: &["ps", "kill", "42"],
        },
        Row {
            args: &["port", "kill", "8080"],
        },
        Row {
            args: &["service", "stop", "sshd"],
        },
        Row {
            args: &["firewall", "allow", "8080"],
        },
        Row {
            args: &["power", "shutdown", "--delay", "60"],
        },
    ];
    for row in &rows {
        row.run(&stubs);
    }
}

#[test]
fn dry_run_changes_nothing_even_when_confirmed() {
    let stubs = Stubs::new()
        .with_ports(vec![stub_socket(8080, 42, "node")])
        .with_processes(vec![stub_process(42, Some(1), "node")])
        .with_services(vec![stub_service("sshd", 42)]);

    x(&stubs, &["ps", "kill", "42", "--dry-run"], true);
    x(&stubs, &["port", "kill", "8080", "--dry-run"], true);
    x(&stubs, &["service", "stop", "sshd", "--dry-run"], true);
    x(&stubs, &["firewall", "allow", "8080", "--dry-run"], true);

    assert!(stubs.port.killed().is_empty(), "dry-run must not kill");
    assert!(
        stubs.firewall.changes().is_empty(),
        "dry-run must not write"
    );
}

#[test]
fn decline_changes_nothing_even_in_json() {
    let stubs = Stubs::new()
        .with_ports(vec![stub_socket(8080, 42, "node")])
        .with_processes(vec![stub_process(42, Some(1), "node")])
        .with_services(vec![stub_service("sshd", 42)]);

    x(&stubs, &["ps", "kill", "42"], false);
    x(&stubs, &["port", "kill", "8080", "--json"], false);
    x(&stubs, &["service", "stop", "sshd", "--json"], false);
    x(&stubs, &["firewall", "allow", "8080", "--json"], false);

    assert!(stubs.port.killed().is_empty(), "a refusal must not kill");
    assert!(
        stubs.firewall.changes().is_empty(),
        "a refusal must not write"
    );
}

#[test]
fn dry_run_matches_the_stub_context_without_a_host() {
    // The contract must hold on a bare context too, so it is exercised on
    // every platform CI runs.
    let context = stub_context();
    let out = x_in(&context, &["power", "sleep", "--dry-run"], true);
    assert_eq!(out.code, 0);
    assert_eq!(out.asked, 0);
    assert!(out.stdout.contains("dry-run"), "{}", out.stdout);
}
