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

pub use commands::dry_run_guard;

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

    /// Render one JSON object per row (newline delimited, stream friendly).
    #[arg(long, global = true, conflicts_with_all = ["json", "plain", "csv", "no_color"])]
    pub jsonl: bool,

    /// Render RFC 4180 CSV with a header row.
    #[arg(long, global = true, conflicts_with_all = ["json", "plain", "jsonl", "no_color"])]
    pub csv: bool,

    /// Never emit ANSI colors.
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Force ANSI colors even when stdout is not a terminal.
    #[arg(long, global = true, conflicts_with = "no_color")]
    pub color: bool,

    /// Show what a destructive command would do, without doing it.
    #[arg(long, global = true)]
    pub dry_run: bool,

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

    /// Files: inspect, open, reveal, trash, copy, move, rename.
    #[command(subcommand)]
    File(commands::file::FileCommand),

    /// Environment variables.
    #[command(subcommand)]
    Env(commands::envpath::EnvCommand),

    /// PATH inspection and edits (this process only).
    #[command(subcommand)]
    Path(commands::envpath::PathCommand),

    /// Resolve a command on PATH (`where` / `which` / `command -v`).
    Which(commands::envpath::WhichArgs),

    /// Login shells.
    #[command(subcommand)]
    Shell(commands::shell::ShellCommand),

    /// Users.
    #[command(subcommand)]
    User(commands::usergroup::UserCommand),

    /// Groups.
    #[command(subcommand)]
    Group(commands::usergroup::GroupCommand),

    /// Clipboard.
    #[command(subcommand)]
    Clipboard(commands::clipboard::ClipboardCommand),

    /// SSH hosts, reachability and connections.
    #[command(subcommand)]
    Ssh(commands::ssh::SshCommand),

    /// Git repository facts.
    #[command(subcommand)]
    Git(commands::gitcmd::GitCommand),

    /// Development toolchains installed here.
    Dev(commands::devtools::DevArgs),

    /// Health-check one command, or sweep the whole environment.
    Doctor(commands::devtools::DoctorArgs),

    /// What kind of project is this directory.
    Project(commands::devtools::ProjectArgs),

    /// Docker containers, images, ports and logs.
    #[command(subcommand)]
    Docker(commands::devtools::DockerCommand),

    /// What this host can actually do: supported / degraded / unsupported.
    Capability(commands::capability::CapabilityArgs),

    /// Environment and system proxy.
    #[command(subcommand)]
    Proxy(commands::proxy::ProxyCommand),

    /// The hosts file: list, look up, add, remove.
    #[command(subcommand)]
    Hosts(commands::hosts::HostsCommand),

    /// Battery and power verbs (sleep / shutdown / reboot).
    #[command(subcommand)]
    Power(commands::powertime::PowerCommand),

    /// Clock facts and time sync.
    #[command(subcommand)]
    Time(commands::powertime::TimeCommand),

    /// Mounted filesystems and mount / unmount.
    #[command(subcommand)]
    Mount(commands::mount::MountCommand),

    /// Who may touch a path (Unix mode bits / Windows ACL).
    #[command(subcommand)]
    Permission(commands::permission::PermissionCommand),

    /// Programs that start at login.
    #[command(subcommand)]
    Startup(commands::startup::StartupCommand),

    /// Scheduled tasks (cron / Task Scheduler / launchd).
    #[command(subcommand)]
    Schedule(commands::schedule::ScheduleCommand),

    /// Generate a shell completion script (bash/zsh/fish/powershell/elvish).
    Completion(commands::completion::CompletionArgs),

    /// Generate roff man pages from the CLI grammar.
    Man(commands::manpage::ManArgs),

    /// Firewall state and port rules.
    #[command(subcommand)]
    Firewall(commands::firewall::FirewallCommand),

    /// The system log: journal / Event Log / unified log, read-only.
    #[command(subcommand)]
    Logs(commands::logs::LogsCommand),

    /// Hardware inventory (USB, Bluetooth, audio, display, camera, HID).
    #[command(subcommand)]
    Device(commands::device::DeviceCommand),

    /// Bluetooth radios, known devices, and the link verbs.
    #[command(subcommand)]
    Bluetooth(commands::bluetooth::BluetoothCommand),

    /// Displays and monitors: topology, modes, scaling.
    #[command(subcommand)]
    Display(commands::display::DisplayCommand),

    /// Open windows: inventory, focus, minimize, maximize.
    #[command(subcommand)]
    Window(commands::window::WindowCommand),

    /// A live stream of system changes: processes, sockets, USB, mounts, services.
    Events(commands::events::EventsArgs),

    /// Certificate facts for a host (TLS handshake through platform tools).
    #[command(subcommand)]
    Cert(commands::netdiag::CertCommand),

    /// Negotiated TLS protocol and cipher for a host.
    Tls(commands::netdiag::TlsArgs),

    /// One HTTP request through the platform's HTTP client.
    Http(commands::netdiag::HttpArgs),

    /// Response headers of one HTTP request.
    Headers(commands::netdiag::HttpArgs),
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
    } else if cli.jsonl {
        OutputFormat::Jsonl
    } else if cli.csv {
        OutputFormat::Csv
    } else if cli.plain {
        OutputFormat::Plain
    } else {
        OutputFormat::Table
    };

    let mut renderer = Renderer::stdout(format, color);
    renderer.set_dry_run(cli.dry_run);
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
            report(&e, color, Some(context.os()));
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
            report(&error, false, None);
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
        Some(Command::File(cmd)) => commands::file::dispatch(context, renderer, confirmer, cmd),
        Some(Command::Env(cmd)) => commands::envpath::dispatch_env(context, renderer, cmd),
        Some(Command::Path(cmd)) => commands::envpath::dispatch_path(context, renderer, cmd),
        Some(Command::Which(args)) => commands::envpath::dispatch_which(context, renderer, args),
        Some(Command::Shell(cmd)) => commands::shell::dispatch(context, renderer, cmd),
        Some(Command::User(cmd)) => commands::usergroup::dispatch_user(context, renderer, cmd),
        Some(Command::Group(cmd)) => commands::usergroup::dispatch_group(context, renderer, cmd),
        Some(Command::Clipboard(cmd)) => commands::clipboard::dispatch(context, renderer, cmd),
        Some(Command::Ssh(cmd)) => commands::ssh::dispatch(context, renderer, cmd),
        Some(Command::Git(cmd)) => commands::gitcmd::dispatch(context, renderer, cmd),
        Some(Command::Dev(args)) => commands::devtools::dispatch_dev(context, renderer, args),
        Some(Command::Doctor(args)) => commands::devtools::dispatch_doctor(context, renderer, args),
        Some(Command::Project(args)) => {
            commands::devtools::dispatch_project(context, renderer, args)
        }
        Some(Command::Docker(cmd)) => commands::devtools::dispatch_docker(context, renderer, cmd),
        Some(Command::Capability(args)) => commands::capability::dispatch(context, renderer, args),
        Some(Command::Proxy(cmd)) => commands::proxy::dispatch(context, renderer, cmd),
        Some(Command::Hosts(cmd)) => commands::hosts::dispatch(context, renderer, cmd),
        Some(Command::Power(cmd)) => {
            commands::powertime::dispatch_power(context, renderer, confirmer, cmd)
        }
        Some(Command::Time(cmd)) => commands::powertime::dispatch_time(context, renderer, cmd),
        Some(Command::Mount(cmd)) => commands::mount::dispatch(context, renderer, confirmer, cmd),
        Some(Command::Permission(cmd)) => commands::permission::dispatch(context, renderer, cmd),
        Some(Command::Startup(cmd)) => {
            commands::startup::dispatch(context, renderer, confirmer, cmd)
        }
        Some(Command::Schedule(cmd)) => {
            commands::schedule::dispatch(context, renderer, confirmer, cmd)
        }
        Some(Command::Completion(args)) => commands::completion::dispatch(context, renderer, args),
        Some(Command::Man(args)) => commands::manpage::dispatch(context, renderer, args),
        Some(Command::Firewall(cmd)) => {
            commands::firewall::dispatch(context, renderer, confirmer, cmd)
        }
        Some(Command::Logs(cmd)) => commands::logs::dispatch(context, renderer, cmd),
        Some(Command::Device(cmd)) => commands::device::dispatch(context, renderer, cmd),
        Some(Command::Bluetooth(cmd)) => {
            commands::bluetooth::dispatch(context, renderer, confirmer, cmd)
        }
        Some(Command::Display(cmd)) => commands::display::dispatch(context, renderer, cmd),
        Some(Command::Window(cmd)) => commands::window::dispatch(context, renderer, confirmer, cmd),
        Some(Command::Events(args)) => commands::events::dispatch(context, renderer, args),
        Some(Command::Cert(cmd)) => commands::netdiag::dispatch_cert(context, renderer, cmd),
        Some(Command::Tls(args)) => commands::netdiag::dispatch_tls(context, renderer, args),
        Some(Command::Http(args)) => commands::netdiag::dispatch_http(context, renderer, args),
        Some(Command::Headers(args)) => {
            commands::netdiag::dispatch_headers(context, renderer, args)
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

/// Print an error the way a user can act on it, including privilege guidance
/// worded for the OS the command actually ran on.
fn report(error: &Error, color: bool, os: Option<x_core::system::OsFamily>) {
    let mut stderr = std::io::stderr();
    let label = if color {
        "\x1b[1;31merror\x1b[0m"
    } else {
        "error"
    };
    let _ = writeln!(stderr, "{label}: {}", error.message());
    if let Some(permission) = error.permission() {
        let hint = match os {
            Some(os) => permission.platform_guidance(os),
            None => permission.guidance(),
        };
        let _ = writeln!(stderr, "hint: {hint}");
    }
    if let Some(source) = error.source_error() {
        let _ = writeln!(stderr, "cause: {source}");
    }
}
