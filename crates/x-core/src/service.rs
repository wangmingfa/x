//! Unified service model.
//!
//! The interesting part is that a service is *not* the same concept on every
//! platform: Windows has the SCM, Linux has systemd, OpenRC, runit or nothing
//! at all, macOS has launchd. x exposes one vocabulary and reports which
//! manager backs it.

use serde::{Deserialize, Serialize};

/// Which service manager the adapter actually found at runtime.
///
/// Adapters must detect this instead of assuming: plenty of Linux containers
/// run without systemd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceManagerType {
    /// Windows Service Control Manager.
    #[cfg_attr(not(windows), allow(dead_code))]
    #[serde(rename = "windows_scm")]
    WindowsScm,
    /// systemd (`systemctl` / D-Bus).
    #[serde(rename = "systemd")]
    Systemd,
    /// OpenRC (`rc-service`).
    #[serde(rename = "openrc")]
    OpenRc,
    /// SysV init scripts.
    #[serde(rename = "sysv")]
    SysV,
    /// Upstart.
    #[serde(rename = "upstart")]
    Upstart,
    /// macOS launchd (`launchctl`).
    #[serde(rename = "launchd")]
    Launchd,
    /// Nothing was detected.
    Unknown,
}

impl ServiceManagerType {
    /// Lowercase identifier.
    pub const fn name(self) -> &'static str {
        match self {
            Self::WindowsScm => "windows_scm",
            Self::Systemd => "systemd",
            Self::OpenRc => "openrc",
            Self::SysV => "sysv",
            Self::Upstart => "upstart",
            Self::Launchd => "launchd",
            Self::Unknown => "unknown",
        }
    }

    /// Whether lifecycle operations are supported by this manager.
    pub const fn supports_control(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// Desired or effective state of a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    /// Running now.
    Running,
    /// Installed but not running.
    Stopped,
    /// Starting.
    Starting,
    /// Stopping.
    Stopping,
    /// Failed to start or crashed.
    Failed,
    /// Installed but disabled at boot.
    Disabled,
    /// Installed and enabled at boot, not running now.
    Enabled,
    /// Enabled at boot and running.
    ActiveEnabled,
    /// Not installed.
    NotFound,
    /// The manager does not report a state.
    Unknown,
}

impl ServiceState {
    /// `true` when the service is currently running.
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::ActiveEnabled)
    }
}

/// Lifecycle verbs accepted by every service manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceAction {
    /// Start a stopped service.
    Start,
    /// Stop a running service.
    Stop,
    /// Stop then start.
    Restart,
    /// Reload configuration without restarting.
    Reload,
    /// Enable at boot.
    Enable,
    /// Disable at boot.
    Disable,
}

/// A service as exposed by the platform manager.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// Identifier accepted by `status`, `start`, ... on this platform.
    pub name: String,
    /// Display name when the platform has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Description when the platform has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Current state.
    pub state: ServiceState,
    /// Main process id when running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Whether the service starts at boot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Manager backing this entry.
    pub manager: ServiceManagerType,
}

/// How many log lines a manager returns when the caller did not ask for a
/// specific count.
pub const DEFAULT_LOG_LINES: usize = 50;

/// One line of a service's recent log output.
///
/// The fields are deliberately loose: journal entries, macOS unified-log
/// records and Windows events do not share a level vocabulary or a timestamp
/// format, and inventing a lossy normalization would be worse than passing the
/// platform's own text through.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceLogEntry {
    /// Timestamp as the platform printed it, when it printed one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Level as the platform named it ("error", "warning", "info", a number),
    /// when the record carried one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// The message text.
    pub message: String,
}

/// The logs fetched for one service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceLogPage {
    /// Service the entries were requested for.
    pub service: String,
    /// Where the entries came from, e.g. `journalctl` or `wevtutil`.
    pub source: String,
    /// Newest first, capped at the requested line count.
    pub entries: Vec<ServiceLogEntry>,
}

/// What running the platform manager's own command line produced.
///
/// This is the documented escape hatch: when `x` has no normalized operation
/// for something, the adapter can hand the arguments to `systemctl`,
/// `launchctl` or `sc` verbatim and show the raw answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeOutput {
    /// Program that was executed.
    pub program: String,
    /// Arguments it received.
    pub args: Vec<String>,
    /// Its exit code, `0` on success.
    pub exit_code: i32,
    /// Everything it wrote to stdout.
    pub stdout: String,
    /// Everything it wrote to stderr.
    pub stderr: String,
}

/// Filter for [`crate::service::ServiceManager::list`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceListOptions {
    /// Only services whose name contains this substring.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    /// Only services in this state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ServiceState>,
    /// Only running services.
    pub running_only: bool,
    /// Maximum number of rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Service capability, normalized across platforms.
pub trait ServiceManager: Send + Sync {
    /// Which manager backs this adapter.
    fn manager_type(&self) -> ServiceManagerType;

    /// List services.
    fn list(&self, options: &ServiceListOptions) -> crate::error::Result<Vec<ServiceInfo>>;

    /// Look up one service.
    fn status(&self, name: &str) -> crate::error::Result<ServiceInfo> {
        let options = ServiceListOptions {
            search: Some(name.to_string()),
            ..Default::default()
        };
        self.list(&options)?
            .into_iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| crate::error::Error::not_found(format!("service `{name}` not found")))
    }

    /// Apply a lifecycle action.
    fn action(&self, name: &str, action: ServiceAction) -> crate::error::Result<()>;

    /// Convenience wrappers so adapters only implement `action`.
    fn start(&self, name: &str) -> crate::error::Result<()> {
        self.action(name, ServiceAction::Start)
    }

    /// See [`ServiceManager::start`].
    fn stop(&self, name: &str) -> crate::error::Result<()> {
        self.action(name, ServiceAction::Stop)
    }

    /// See [`ServiceManager::start`].
    fn restart(&self, name: &str) -> crate::error::Result<()> {
        self.action(name, ServiceAction::Restart)
    }

    /// Recent log lines for one service, newest first.
    ///
    /// Adapters implement this only where the platform has a real log source;
    /// the default reports that honestly instead of returning an empty page
    /// that looks like "no logs".
    fn logs(&self, _name: &str, _limit: Option<usize>) -> crate::error::Result<ServiceLogPage> {
        Err(crate::error::Error::unsupported(
            "this service manager has no log source x can read",
        ))
    }

    /// Run the platform manager's own command line with these arguments.
    ///
    /// Escape hatch for operations `x` does not model. Adapters that have a
    /// native manager command override this.
    fn native(&self, _args: &[String]) -> crate::error::Result<NativeOutput> {
        Err(crate::error::Error::unsupported(
            "no native service command is available on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_names_are_stable() {
        assert_eq!(ServiceManagerType::Systemd.name(), "systemd");
        assert_eq!(ServiceManagerType::Launchd.name(), "launchd");
        assert!(ServiceManagerType::Systemd.supports_control());
        assert!(!ServiceManagerType::Unknown.supports_control());
    }

    #[test]
    fn state_predicates() {
        assert!(ServiceState::Running.is_running());
        assert!(ServiceState::ActiveEnabled.is_running());
        assert!(!ServiceState::Stopped.is_running());
    }

    #[test]
    fn manager_type_serde_is_snake_case() {
        let json = serde_json::to_string(&ServiceManagerType::OpenRc).unwrap();
        assert_eq!(json, "\"openrc\"");
    }

    #[test]
    fn unset_logs_and_native_report_unsupported() {
        struct Bare;
        impl ServiceManager for Bare {
            fn manager_type(&self) -> ServiceManagerType {
                ServiceManagerType::Unknown
            }
            fn list(
                &self,
                _options: &ServiceListOptions,
            ) -> crate::error::Result<Vec<ServiceInfo>> {
                Ok(Vec::new())
            }
            fn action(&self, _name: &str, _action: ServiceAction) -> crate::error::Result<()> {
                Ok(())
            }
        }
        let bare = Bare;
        assert_eq!(
            bare.logs("ssh", None)
                .expect_err("default must be unsupported")
                .kind(),
            crate::error::ErrorKind::Unsupported
        );
        assert_eq!(
            bare.native(&["status".into()])
                .expect_err("default must be unsupported")
                .kind(),
            crate::error::ErrorKind::Unsupported
        );
    }

    #[test]
    fn log_entries_drop_absent_fields_in_json() {
        let entry = ServiceLogEntry {
            timestamp: None,
            level: None,
            message: "plain".into(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert_eq!(json, r#"{"message":"plain"}"#);
    }
}
