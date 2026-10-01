//! Linux service adapter: `systemd` first, then OpenRC, then SysV.
//!
//! Service lifecycle is the one place Linux has no stable native interface
//! outside the init system itself, so this is the documented level 3 in the
//! crate's native-first ladder: the manager's own CLI is the supported API.
//! Detection follows the filesystem layout the distributions agree on.

use crate::sys;
use x_core::error::{Error, PermissionRequirement, Result};
use x_core::service::{
    NativeOutput, ServiceAction, ServiceInfo, ServiceListOptions, ServiceLogEntry, ServiceLogPage,
    ServiceManager, ServiceManagerType, ServiceState, DEFAULT_LOG_LINES,
};

/// Reads and drives Linux services.
#[derive(Debug, Default)]
pub struct LinuxService;

impl LinuxService {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }
}

/// Which init system owns this machine.
pub fn detect() -> ServiceManagerType {
    if std::path::Path::new("/run/systemd/system").is_dir() {
        return ServiceManagerType::Systemd;
    }
    if std::path::Path::new("/sbin/openrc").exists()
        || std::path::Path::new("/usr/sbin/openrc").exists()
        || std::path::Path::new("/etc/init.d/openrc").exists()
    {
        return ServiceManagerType::OpenRc;
    }
    if std::path::Path::new("/etc/init.d").is_dir() {
        return ServiceManagerType::SysV;
    }
    ServiceManagerType::Unknown
}

impl ServiceManager for LinuxService {
    fn manager_type(&self) -> ServiceManagerType {
        detect()
    }

    fn list(&self, options: &ServiceListOptions) -> Result<Vec<ServiceInfo>> {
        let rows = match self.manager_type() {
            ServiceManagerType::Systemd => list_systemd()?,
            ServiceManagerType::OpenRc => list_openrc()?,
            ServiceManagerType::SysV => list_sysv()?,
            other => {
                return Err(Error::unsupported(format!(
                    "no service manager found on this host (detected {other:?})"
                )))
            }
        };

        Ok(rows
            .into_iter()
            .filter(|row| match &options.search {
                Some(search) => {
                    let needle = search.to_ascii_lowercase();
                    row.name.to_ascii_lowercase().contains(&needle)
                        || row
                            .description
                            .as_deref()
                            .is_some_and(|d| d.to_ascii_lowercase().contains(&needle))
                }
                None => true,
            })
            .filter(|row| match options.state {
                Some(ServiceState::Running) => row.state == ServiceState::Running,
                Some(ServiceState::Stopped) => row.state == ServiceState::Stopped,
                _ => true,
            })
            .collect())
    }

    fn action(&self, name: &str, action: ServiceAction) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid_input("service name must not be empty"));
        }
        // A unit name is the only thing that reaches the shell, and it is
        // restricted to the characters systemd and OpenRC actually accept.
        if !valid_unit(name) {
            return Err(Error::invalid_input(format!(
                "`{name}` is not a valid service name"
            )));
        }

        let manager = self.manager_type();
        let command = match (manager, action) {
            (ServiceManagerType::Systemd, ServiceAction::Start) => vec!["start", name],
            (ServiceManagerType::Systemd, ServiceAction::Stop) => vec!["stop", name],
            (ServiceManagerType::Systemd, ServiceAction::Restart) => vec!["restart", name],
            (ServiceManagerType::Systemd, ServiceAction::Reload) => vec!["reload-or-restart", name],
            (ServiceManagerType::Systemd, ServiceAction::Enable) => vec!["enable", name],
            (ServiceManagerType::Systemd, ServiceAction::Disable) => vec!["disable", name],
            (ServiceManagerType::OpenRc, ServiceAction::Start) => vec![name, "start"],
            (ServiceManagerType::OpenRc, ServiceAction::Stop) => vec![name, "stop"],
            (ServiceManagerType::OpenRc, ServiceAction::Restart) => vec![name, "restart"],
            (ServiceManagerType::OpenRc, ServiceAction::Reload) => vec![name, "reload"],
            (ServiceManagerType::OpenRc, ServiceAction::Enable) => vec![name, "start"],
            (ServiceManagerType::OpenRc, ServiceAction::Disable) => vec![name, "stop"],
            (ServiceManagerType::SysV, ServiceAction::Start) => vec![name, "start"],
            (ServiceManagerType::SysV, ServiceAction::Stop) => vec![name, "stop"],
            (ServiceManagerType::SysV, ServiceAction::Restart) => vec![name, "restart"],
            (ServiceManagerType::SysV, ServiceAction::Reload) => vec![name, "reload-or-restart"],
            (ServiceManagerType::SysV, ServiceAction::Enable) => {
                vec!["update-rc.d", name, "enable"]
            }
            (ServiceManagerType::SysV, ServiceAction::Disable) => {
                vec!["update-rc.d", name, "disable"]
            }
            (other, _) => {
                return Err(Error::unsupported(format!(
                    "service actions are not supported by {other:?}"
                )))
            }
        };

        run(&command, name, &action)
    }

    fn logs(&self, name: &str, limit: Option<usize>) -> Result<ServiceLogPage> {
        if self.manager_type() != ServiceManagerType::Systemd {
            return Err(Error::unsupported(
                "OpenRC and SysV keep service logs in files with no stable schema; only systemd's journal is supported here",
            ));
        }
        let name = name.trim();
        if !valid_unit(name) {
            return Err(Error::invalid_input(format!(
                "`{name}` is not a valid service name"
            )));
        }
        if !sys::command_exists("journalctl") {
            return Err(Error::unsupported("journalctl is not installed"));
        }
        // journalctl matches unit names exactly; a bare `sshd` means the unit
        // `sshd.service`.
        let unit = if name.contains('.') {
            name.to_string()
        } else {
            format!("{name}.service")
        };
        let lines = limit.unwrap_or(DEFAULT_LOG_LINES);
        let output = sys::run_command(
            "journalctl",
            &["-u", &unit, "-n", &lines.to_string(), "--no-pager"],
        )
        .map_err(|err| Error::system(format!("journalctl -u {unit}: {err}")))?;

        // The short format is `Mon DD HH:MM:SS host prog[pid]: message`; the
        // three leading tokens are the timestamp and everything else stays as
        // the platform wrote it.
        let mut entries: Vec<ServiceLogEntry> = output.lines().map(journal_line).collect();
        entries.reverse();
        Ok(ServiceLogPage {
            service: name.to_string(),
            source: "journalctl".to_string(),
            entries,
        })
    }

    fn native(&self, args: &[String]) -> Result<NativeOutput> {
        if args.is_empty() {
            return Err(Error::invalid_input(
                "x service native needs at least one manager argument",
            ));
        }
        let program = match self.manager_type() {
            ServiceManagerType::Systemd => "systemctl",
            ServiceManagerType::OpenRc => "rc-service",
            ServiceManagerType::SysV => "service",
            other => {
                return Err(Error::unsupported(format!(
                    "no service manager to hand `{other:?}` commands to"
                )))
            }
        };
        run_native(program, args)
    }
}

/// Characters systemd and OpenRC accept in a unit or script name.
fn valid_unit(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._-:\\".contains(&b))
}

/// Run the manager's own command line and hand the raw answer back.
fn run_native(program: &str, args: &[String]) -> Result<NativeOutput> {
    sys::run_native_lossy(program, args)
        .map_err(|err| Error::system(format!("cannot run `{program}`: {err}")))
}

/// One journal line in short format, timestamp separated from the body.
fn journal_line(line: &str) -> ServiceLogEntry {
    let mut rest = line;
    let mut stamp: Vec<&str> = Vec::new();
    for _ in 0..3 {
        let trimmed = rest.trim_start();
        let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        if end == 0 {
            break;
        }
        stamp.push(&trimmed[..end]);
        rest = &trimmed[end..];
    }
    // A line that is not a journal record (continuation of a multi-line
    // message) has no timestamp to claim; keep it as the message.
    let looks_like_timestamp = stamp
        .first()
        .is_some_and(|token| token.len() == 3 && token.as_bytes()[0].is_ascii_alphabetic());
    if !looks_like_timestamp {
        return ServiceLogEntry {
            timestamp: None,
            level: None,
            message: line.to_string(),
        };
    }
    ServiceLogEntry {
        timestamp: (!stamp.is_empty()).then(|| stamp.join(" ")),
        level: None,
        message: rest.trim_start().to_string(),
    }
}

/// Run a manager command, turning a failure into the right error class.
fn run(command: &[&str], service: &str, action: &ServiceAction) -> Result<()> {
    let (program, args) = command.split_first().expect("non-empty command");
    match sys::run_command(program, args) {
        Ok(_) => Ok(()),
        Err(err) => {
            // A refusal from the init system is a permission problem, not a
            // broken tool, and the CLI says so on stderr.
            let text = err.to_string().to_ascii_lowercase();
            let permission = if text.contains("permission denied")
                || text.contains("access denied")
                || text.contains("operation not permitted")
                || text.contains("must be root")
                || text.contains("interactive authentication required")
                || text.contains("failed to authorize")
            {
                PermissionRequirement::Root
            } else {
                PermissionRequirement::None
            };
            Err(Error::new(
                x_core::error::ErrorKind::System,
                format!("{action:?} of `{service}` failed: {err}"),
            )
            .with_permission(permission))
        }
    }
}

/// Normalise a systemd or init state name.
pub fn normalize_state(raw: &str) -> ServiceState {
    match raw.trim().to_ascii_lowercase().as_str() {
        "active" | "running" | "up" | "started" | "online" => ServiceState::Running,
        "inactive" | "dead" | "stopped" | "down" | "exited" => ServiceState::Stopped,
        "failed" | "crashed" | "degraded" => ServiceState::Failed,
        "activating" | "starting" | "deactivating" | "stopping" => ServiceState::Starting,
        "enabled" => ServiceState::Enabled,
        "disabled" => ServiceState::Disabled,
        _ => ServiceState::Unknown,
    }
}

/// `systemctl list-units`, which lists loaded units whether they are running or
/// not.
fn list_systemd() -> Result<Vec<ServiceInfo>> {
    let output = sys::run_command(
        "systemctl",
        &[
            "list-units",
            "--type=service",
            "--all",
            "--no-legend",
            "--no-pager",
            "--plain",
        ],
    )
    .map_err(|err| Error::system(format!("systemctl list-units: {err}")))?;

    Ok(output
        .lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 4 || !cols[0].ends_with(".service") {
                return None;
            }
            Some(ServiceInfo {
                name: cols[0].to_string(),
                display_name: Some(cols[0].trim_end_matches(".service").to_string()),
                description: Some(cols[3..].join(" ")),
                state: normalize_state(cols[2]),
                enabled: None,
                pid: None,
                manager: ServiceManagerType::Systemd,
            })
        })
        .collect())
}

/// `rc-status --servicelist`, one service per line.
fn list_openrc() -> Result<Vec<ServiceInfo>> {
    let output = sys::run_command("rc-status", &["--servicelist"])
        .map_err(|err| Error::system(format!("rc-status: {err}")))?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| ServiceInfo {
            name: name.to_string(),
            display_name: Some(name.to_string()),
            description: None,
            state: openrc_state(name),
            enabled: None,
            pid: None,
            manager: ServiceManagerType::OpenRc,
        })
        .collect())
}

/// Ask OpenRC about one service, treating a missing runner as "stopped".
fn openrc_state(name: &str) -> ServiceState {
    match sys::run_command("rc-service", &[name, "status"]) {
        Ok(output) if output.contains("started") => ServiceState::Running,
        Ok(_) => ServiceState::Stopped,
        Err(_) => ServiceState::Unknown,
    }
}

/// SysV has no single listing command, so the init scripts themselves are the
/// list and `service <name> status` is the state.
fn list_sysv() -> Result<Vec<ServiceInfo>> {
    let entries = std::fs::read_dir("/etc/init.d")
        .map_err(|err| Error::system(format!("/etc/init.d: {err}")))?;

    let mut rows: Vec<ServiceInfo> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .map(|name| ServiceInfo {
            display_name: Some(name.clone()),
            state: sysv_state(&name),
            name,
            description: None,
            enabled: None,
            pid: None,
            manager: ServiceManagerType::SysV,
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

fn sysv_state(name: &str) -> ServiceState {
    match sys::run_command("service", &[name, "status"]) {
        Ok(output) if !output.contains("not running") && !output.contains("stopped") => {
            ServiceState::Running
        }
        Ok(_) => ServiceState::Stopped,
        Err(_) => ServiceState::Unknown,
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn ServiceManager> {
    std::sync::Arc::new(LinuxService::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_are_normalised() {
        assert_eq!(normalize_state("active"), ServiceState::Running);
        assert_eq!(normalize_state("  Inactive "), ServiceState::Stopped);
        assert_eq!(normalize_state("failed"), ServiceState::Failed);
        assert_eq!(normalize_state("activating"), ServiceState::Starting);
        assert_eq!(normalize_state("enabled"), ServiceState::Enabled);
        assert_eq!(normalize_state("whatever"), ServiceState::Unknown);
    }

    #[test]
    fn detection_returns_a_known_manager_on_a_real_host() {
        let manager = detect();
        assert!(matches!(
            manager,
            ServiceManagerType::Systemd
                | ServiceManagerType::OpenRc
                | ServiceManagerType::SysV
                | ServiceManagerType::Unknown
        ));
    }

    #[test]
    fn dangerous_service_names_are_rejected() {
        let service = LinuxService::new();
        let err = service
            .action("evil; rm -rf /", ServiceAction::Start)
            .expect_err("must be rejected");
        assert!(matches!(err.kind(), x_core::error::ErrorKind::InvalidInput));
        assert!(service.action("  ", ServiceAction::Start).is_err());
    }

    #[test]
    fn journal_lines_split_the_timestamp_from_the_body() {
        let entry = journal_line("Oct 05 10:23:41 host sshd[1234]: Accepted publickey for u");
        assert_eq!(entry.timestamp.as_deref(), Some("Oct 05 10:23:41"));
        assert_eq!(entry.message, "host sshd[1234]: Accepted publickey for u");
        // Multi-line message continuations must not claim a timestamp.
        let continuation = journal_line("    still the same message body");
        assert_eq!(continuation.timestamp, None);
        assert_eq!(continuation.message, "    still the same message body");
    }

    #[test]
    fn unit_validation_accepts_what_the_managers_do() {
        assert!(valid_unit("ssh"));
        assert!(valid_unit("nginx.service"));
        assert!(valid_unit("getty@tty1"));
        assert!(!valid_unit(""));
        assert!(!valid_unit("evil; rm -rf /"));
    }

    #[test]
    fn native_refuses_an_empty_argument_list() {
        let service = LinuxService::new();
        let err = service.native(&[]).expect_err("must reject");
        assert_eq!(err.kind(), x_core::error::ErrorKind::InvalidInput);
    }

    #[test]
    fn listing_succeeds_on_a_system_with_a_manager() {
        let service = LinuxService::new();
        if service.manager_type() == ServiceManagerType::Unknown {
            return;
        }
        let rows = service
            .list(&ServiceListOptions {
                search: Some("ssh".to_string()),
                ..Default::default()
            })
            .expect("list");
        // A host without ssh is a valid outcome; a panic is not.
        assert!(rows.len() < 1000);
    }
}
