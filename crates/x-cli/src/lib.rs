//! `x` command line interface.
//!
//! ```text
//! x-cli  ──> x-core traits ──> x-platform ──> macOS / Linux / Windows
//! ```
//!
//! Every command is a thin translation from arguments to a
//! [`x_core::context::SystemContext`] call plus rendering. There is no logic
//! here that a TUI could not reuse, and no OS specific code at all: the CLI
//! never learns whether it runs on Windows, Linux or macOS.
//!
//! Destructive commands (`port kill`, `ps kill`, `service stop`, ...) always
//! show what they are about to do and require a confirmation unless `--yes` is
//! passed, so scripts stay non-interactive and humans stay safe.

#![forbid(unsafe_code)]

pub mod commands;
pub mod format;

use std::io::Write;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use x_core::error::{Error, Result};
use x_core::KillSignal;

use crate::format::{should_colorize, Confirmer, OutputFormat, Renderer, StdinConfirmer};

/// Cross platform process, port, network, service and system inspector.
#[derive(Debug, Parser)]
#[command(
    name = "x",
    version,
    about,
    long_about = "x inspects processes, ports, network, services, disks and system health \
                  with one consistent vocabulary on Windows, Linux and macOS.",
    propagate_version = true
)]
pub struct Cli {
    /// Render machine readable JSON.
    #[arg(long, global = true, conflicts_with_all = ["plain", "no_color"])]
    pub json: bool,

    /// Render tab separated records without a header, for scripts.
    #[arg(long, global = true, conflicts_with = "no_color")]
    pub plain: bool,

    /// Never emit ANSI colors.
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Force ANSI colors even when stdout is not a terminal.
    #[arg(long, global = true, conflicts_with = "no_color")]
    pub color: bool,

    /// Print adapter and contract versions.
    #[arg(long, global = true)]
    pub version_info: bool,

    /// Subcommand. Defaults to `port list`, because "what is on this port" is
    /// the question that made this tool.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Top level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Sockets and who owns them. `x port` lists listening sockets.
    Port(commands::port::PortArgs),

    /// Processes. `x ps` lists them, busiest first.
    Ps(commands::ps::PsArgs),

    /// System facts, CPU and memory.
    #[command(subcommand)]
    Sys(commands::sys::SysCommand),

    /// Interfaces, addresses, routes and DNS.
    #[command(subcommand)]
    Net(commands::net::NetCommand),

    /// Mounted filesystems.
    #[command(subcommand)]
    Disk(commands::disk::DiskCommand),

    /// Services.
    #[command(subcommand)]
    Service(commands::service::ServiceCommand),
}

/// Signal to deliver to a process.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum SignalArg {
    /// Graceful termination.
    Term,
    /// Hard kill.
    Kill,
    /// Interrupt.
    Interrupt,
    /// Hang up.
    Hangup,
}

impl From<SignalArg> for KillSignal {
    fn from(value: SignalArg) -> Self {
        match value {
            SignalArg::Term => KillSignal::Terminate,
            SignalArg::Kill => KillSignal::Kill,
            SignalArg::Interrupt => KillSignal::Interrupt,
            SignalArg::Hangup => KillSignal::Hangup,
        }
    }
}

/// Run the parsed command against a context the caller built.
///
/// This is the entry point a composition root wants: `x` builds the context
/// once and both front ends share it.
pub fn run_with(context: &x_core::SystemContext, cli: Cli) -> i32 {
    let color = should_colorize(if cli.no_color {
        Some(false)
    } else if cli.color {
        Some(true)
    } else {
        None
    });
    let format = if cli.json {
        OutputFormat::Json
    } else if cli.plain {
        OutputFormat::Plain
    } else {
        OutputFormat::Table
    };

    let mut renderer = Renderer::stdout(format, color);
    let mut confirmer: Box<dyn Confirmer> = Box::new(StdinConfirmer);

    let outcome = dispatch(context, &cli, &mut renderer, &mut *confirmer);
    let flush = renderer.flush();

    match outcome {
        Ok(code) => {
            if let Err(e) = flush {
                eprintln!("x: {e}");
                return 1;
            }
            code
        }
        Err(e) => {
            report(&e, color);
            e.exit_code()
        }
    }
}

/// Run the parsed command against a context this crate built.
///
/// Useful for embedders; `x` itself passes its own context so that the platform
/// lives in exactly one place.
pub fn run(cli: Cli) -> i32 {
    match x_platform::create_context() {
        Ok(context) => run_with(&context, cli),
        Err(error) => {
            report(&error, false);
            error.exit_code()
        }
    }
}

/// Parse the process arguments and run, returning the exit code.
///
/// This is the only entry point a binary needs, which keeps `clap` out of
/// `x-app` and makes the whole front end reachable from one function.
pub fn from_args() -> i32 {
    run(Cli::parse())
}

/// Parse the process arguments and run them against a context the caller built.
pub fn from_args_with(context: &x_core::SystemContext) -> i32 {
    run_with(context, Cli::parse())
}

/// Run a pre-parsed command, for tests and for embedders.
pub fn main_with(cli: Cli) -> ExitCode {
    ExitCode::from(u8::try_from(run(cli)).unwrap_or(1))
}

fn dispatch(
    context: &x_core::SystemContext,
    cli: &Cli,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
) -> Result<i32> {
    if cli.version_info {
        return commands::version(renderer);
    }

    match &cli.command {
        None => commands::port::list(context, renderer, &Default::default()),
        Some(Command::Port(args)) => commands::port::dispatch(context, renderer, confirmer, args),
        Some(Command::Ps(args)) => commands::ps::dispatch(context, renderer, confirmer, args),
        Some(Command::Sys(cmd)) => commands::sys::dispatch(context, renderer, cmd),
        Some(Command::Net(cmd)) => commands::net::dispatch(context, renderer, cmd),
        Some(Command::Disk(cmd)) => commands::disk::dispatch(context, renderer, cmd),
        Some(Command::Service(cmd)) => {
            commands::service::dispatch(context, renderer, confirmer, cmd)
        }
    }
}

/// Run a parsed command against `context`.
///
/// The context is a parameter, not something this crate builds: that is what
/// keeps the CLI free of platform knowledge and what makes every command
/// testable with `x_core::testing::Stubs`.
pub fn execute(
    context: &x_core::SystemContext,
    cli: &Cli,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
) -> Result<i32> {
    dispatch(context, cli, renderer, confirmer)
}

/// Print an error the way a user can act on it, including privilege guidance.
fn report(error: &Error, color: bool) {
    let mut stderr = std::io::stderr();
    let label = if color {
        "\x1b[1;31merror\x1b[0m"
    } else {
        "error"
    };
    let _ = writeln!(stderr, "{label}: {}", error.message());
    if let Some(permission) = error.permission() {
        let _ = writeln!(stderr, "hint: {}", permission.guidance());
    }
    if let Some(source) = error.source_error() {
        let _ = writeln!(stderr, "cause: {source}");
    }
}
