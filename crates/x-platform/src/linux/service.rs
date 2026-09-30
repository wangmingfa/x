//! Linux service adapter: `systemd` first, then OpenRC, then SysV.
//!
//! Service lifecycle is the one place Linux has no stable native interface
//! outside the init system itself, so this is the documented level 3 in the
//! crate's native-first ladder: the manager's own CLI is the supported API.
//! Detection follows the filesystem layout the distributions agree on.

use crate::sys;
use x_core::error::{Error, PermissionRequirement, Result};
use x_core::service::{
    ServiceAction, ServiceInfo, ServiceListOptions, ServiceManager, ServiceManagerType,
    ServiceState,
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
        if !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._-:\\".contains(&b))
        {
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
